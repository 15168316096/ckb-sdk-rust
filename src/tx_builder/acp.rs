use std::collections::HashSet;

use ckb_types::{
    core::{TransactionBuilder, TransactionView},
    packed::{CellInput, Script},
    prelude::*,
};

use super::{TxBuilder, TxBuilderError};
use crate::traits::{
    CellCollector, CellDepResolver, CellQueryOptions, HeaderDepResolver,
    TransactionDependencyProvider,
};
use crate::types::ScriptId;

#[derive(Clone, Debug)]
pub struct AcpTransferReceiver {
    lock_script: Script,
    capacity: u64,
}
/// Transfer capacity to already exists acp cell, the type script and cell data
/// will be copied.
pub struct AcpTransferBuilder {
    receivers: Vec<AcpTransferReceiver>,
}

impl TxBuilder for AcpTransferBuilder {
    fn build_base(
        &self,
        cell_collector: &mut dyn CellCollector,
        cell_dep_resolver: &dyn CellDepResolver,
        _header_dep_resolver: &dyn HeaderDepResolver,
        _tx_dep_provider: &dyn TransactionDependencyProvider,
    ) -> Result<TransactionView, TxBuilderError> {
        #[allow(clippy::mutable_key_type)]
        let mut cell_deps = HashSet::new();
        let mut inputs = Vec::new();
        let mut outputs = Vec::new();
        let mut outputs_data = Vec::new();
        for receiver in &self.receivers {
            let query = CellQueryOptions::new_lock(receiver.lock_script.clone());
            let (cells, input_capacity) = cell_collector.collect_live_cells(&query, true)?;
            if cells.is_empty() {
                return Err(TxBuilderError::Other(
                    format!(
                        "can not found cell by lock script: {:?}",
                        receiver.lock_script
                    )
                    .into(),
                ));
            }
            let input_cell = &cells[0];
            let input = CellInput::new(input_cell.out_point.clone(), 0);
            let output_capacity = input_capacity + receiver.capacity;
            let output = input_cell
                .output
                .clone()
                .as_builder()
                .capacity(output_capacity.pack())
                .build();
            let output_data = input_cell.output_data.clone();

            let lock_script_id = ScriptId::from(&receiver.lock_script);
            let lock_cell_dep = cell_dep_resolver
                .resolve(&lock_script_id)
                .ok_or(TxBuilderError::ResolveCellDepFailed(lock_script_id))?;
            cell_deps.insert(lock_cell_dep);
            if let Some(type_script) = input_cell.output.type_().to_opt() {
                let script_id = ScriptId::from(&type_script);
                let cell_dep = cell_dep_resolver
                    .resolve(&script_id)
                    .ok_or(TxBuilderError::ResolveCellDepFailed(script_id))?;
                cell_deps.insert(cell_dep);
            }

            inputs.push(input);
            outputs.push(output);
            outputs_data.push(output_data.pack());
        }
        Ok(TransactionBuilder::default()
            .set_cell_deps(cell_deps.into_iter().collect())
            .set_inputs(inputs)
            .set_outputs(outputs)
            .set_outputs_data(outputs_data)
            .build())
    }
}
