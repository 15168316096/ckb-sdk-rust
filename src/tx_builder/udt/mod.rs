mod sudt;
mod xudt;

use ckb_types::{
    bytes::{BufMut, Bytes, BytesMut},
    core::{Capacity, TransactionBuilder, TransactionView},
    packed::{CellDep, CellInput, CellOutput, Script},
    prelude::*,
};
use std::collections::HashSet;

use super::{TransferAction, TxBuilder, TxBuilderError};
use crate::traits::{
    CellCollector, CellDepResolver, CellQueryOptions, HeaderDepResolver,
    TransactionDependencyProvider, ValueRangeOption,
};
use crate::types::ScriptId;

pub use xudt::xudt_rce;

/// The udt issue type
#[derive(Debug, Eq, PartialEq, Hash, Clone)]
pub enum UdtIssueType {
    Sudt,
    /// The parameter is <xudt args>
    Xudt(Bytes),
}

/// The udt issue/transfer receiver
#[derive(Debug, Eq, PartialEq, Hash, Clone)]
pub struct UdtTargetReceiver {
    pub action: TransferAction,

    /// The lock script set to this udt cell, if `action` is `Update` will query
    /// input cell by this lock script.
    pub lock_script: Script,

    /// The capacity set to this udt cell when `action` is TransferAction::Create
    pub capacity: Option<u64>,

    /// The amount to issue/transfer
    pub amount: u128,

    /// Only for <xudt data> and only used when action == TransferAction::Create
    pub extra_data: Option<Bytes>,
}

pub struct ReceiverBuildOutput {
    pub input: Option<CellInput>,
    pub cell_dep: Option<CellDep>,
    pub output: CellOutput,
    pub output_data: Bytes,
}

impl UdtTargetReceiver {
    pub fn build(
        &self,
        type_script: &Script,
        cell_collector: &mut dyn CellCollector,
        cell_dep_resolver: &dyn CellDepResolver,
    ) -> Result<ReceiverBuildOutput, TxBuilderError> {
        match self.action {
            TransferAction::Create => {
                let data_len = self
                    .extra_data
                    .as_ref()
                    .map(|data| data.len())
                    .unwrap_or_default()
                    + 16;
                let mut data = BytesMut::with_capacity(data_len);
                data.put(&self.amount.to_le_bytes()[..]);
                if let Some(extra_data) = self.extra_data.as_ref() {
                    data.put(extra_data.as_ref());
                }

                let base_output = CellOutput::new_builder()
                    .lock(self.lock_script.clone())
                    .type_(Some(type_script.clone()).pack())
                    .build();
                let base_occupied_capacity = base_output
                    .occupied_capacity(Capacity::bytes(data_len).unwrap())
                    .unwrap()
                    .as_u64();
                let final_capacity = if let Some(capacity) = self.capacity.as_ref() {
                    if *capacity >= base_occupied_capacity {
                        *capacity
                    } else {
                        return Err(TxBuilderError::Other(
                            format!(
                                "Not enough capacity to hold a receiver cell, min: {}, actual: {}",
                                base_occupied_capacity, *capacity,
                            )
                            .into(),
                        ));
                    }
                } else {
                    base_occupied_capacity
                };
                let output = base_output
                    .as_builder()
                    .capacity(final_capacity.pack())
                    .build();
                Ok(ReceiverBuildOutput {
                    input: None,
                    cell_dep: None,
                    output,
                    output_data: data.freeze(),
                })
            }
            TransferAction::Update => {
                let receiver_query = {
                    let mut query = CellQueryOptions::new_lock(self.lock_script.clone());
                    query.secondary_script = Some(type_script.clone());
                    query.data_len_range = Some(ValueRangeOption::new_min(16));
                    query
                };
                let (receiver_cells, _) =
                    cell_collector.collect_live_cells(&receiver_query, true)?;
                if receiver_cells.is_empty() {
                    return Err(TxBuilderError::Other(
                        format!(
                            "update receiver cell failed, cell not found, lock={:?}",
                            self.lock_script
                        )
                        .into(),
                    ));
                }

                let receiver_script_id = ScriptId::from(&self.lock_script);
                let receiver_cell_dep = cell_dep_resolver
                    .resolve(&receiver_script_id)
                    .ok_or(TxBuilderError::ResolveCellDepFailed(receiver_script_id))?;

                let mut amount_bytes = [0u8; 16];
                let receiver_cell = &receiver_cells[0];
                amount_bytes.copy_from_slice(&receiver_cell.output_data.as_ref()[0..16]);
                let old_amount = u128::from_le_bytes(amount_bytes);
                let new_amount = old_amount + self.amount;
                let mut new_data = receiver_cell.output_data.as_ref().to_vec();
                new_data[0..16].copy_from_slice(&new_amount.to_le_bytes()[..]);
                let output_data = Bytes::from(new_data);

                let input = CellInput::new(receiver_cell.out_point.clone(), 0);
                Ok(ReceiverBuildOutput {
                    input: Some(input),
                    cell_dep: Some(receiver_cell_dep),
                    output: receiver_cell.output.clone(),
                    output_data,
                })
            }
        }
    }
}

/// The udt issue transaction builder
pub struct UdtIssueBuilder {
    /// The udt type (sudt/xudt)
    pub udt_type: UdtIssueType,

    /// The sudt/xudt script id
    pub script_id: ScriptId,

    /// We will collect a cell from owner, there must exists a cell that:
    ///   * type script is None
    ///   * data field is empty
    ///   * is mature
    pub owner: Script,

    /// The receivers
    pub receivers: Vec<UdtTargetReceiver>,
}

impl TxBuilder for UdtIssueBuilder {
    fn build_base(
        &self,
        cell_collector: &mut dyn CellCollector,
        cell_dep_resolver: &dyn CellDepResolver,
        _header_dep_resolver: &dyn HeaderDepResolver,
        _tx_dep_provider: &dyn TransactionDependencyProvider,
    ) -> Result<TransactionView, TxBuilderError> {
        // Build inputs
        let owner_query = {
            let mut query = CellQueryOptions::new_lock(self.owner.clone());
            query.data_len_range = Some(ValueRangeOption::new_exact(0));
            query
        };

        let (owner_cells, _) = cell_collector.collect_live_cells(&owner_query, true)?;
        if owner_cells.is_empty() {
            return Err(TxBuilderError::Other(
                "owner cell not found".to_string().into(),
            ));
        }
        let mut inputs = vec![CellInput::new(owner_cells[0].out_point.clone(), 0)];

        // Build output type script
        let owner_lock_hash = self.owner.calc_script_hash();
        let type_script_args = match &self.udt_type {
            UdtIssueType::Sudt => owner_lock_hash.as_bytes(),
            UdtIssueType::Xudt(extra_args) => {
                let mut data = BytesMut::with_capacity(32 + extra_args.len());
                data.put(owner_lock_hash.as_slice());
                data.put(extra_args.as_ref());
                data.freeze()
            }
        };
        let type_script = Script::new_builder()
            .code_hash(self.script_id.code_hash.pack())
            .hash_type(self.script_id.hash_type.into())
            .args(type_script_args.pack())
            .build();

        let owner_script_id = ScriptId::from(&self.owner);
        let owner_cell_dep = cell_dep_resolver
            .resolve(&owner_script_id)
            .ok_or(TxBuilderError::ResolveCellDepFailed(owner_script_id))?;
        let udt_cell_dep = cell_dep_resolver
            .resolve(&self.script_id)
            .ok_or_else(|| TxBuilderError::ResolveCellDepFailed(self.script_id.clone()))?;
        #[allow(clippy::mutable_key_type)]
        let mut cell_deps = HashSet::new();
        cell_deps.insert(owner_cell_dep);
        cell_deps.insert(udt_cell_dep);

        // Build outputs, outputs_data, cell_deps
        let mut outputs = Vec::new();
        let mut outputs_data = Vec::new();
        for receiver in &self.receivers {
            let ReceiverBuildOutput {
                input,
                cell_dep,
                output,
                output_data,
            } = receiver.build(&type_script, cell_collector, cell_dep_resolver)?;
            if let Some(input) = input {
                inputs.push(input);
            }
            if let Some(cell_dep) = cell_dep {
                cell_deps.insert(cell_dep);
            }

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

pub struct UdtTransferBuilder {
    /// The udt type script
    pub type_script: Script,

    /// Sender's lock script (we will asume there is only one udt cell identify
    /// by `type_script` and `sender`)
    pub sender: Script,

    /// The transfer receivers
    pub receivers: Vec<UdtTargetReceiver>,
}

impl TxBuilder for UdtTransferBuilder {
    fn build_base(
        &self,
        cell_collector: &mut dyn CellCollector,
        cell_dep_resolver: &dyn CellDepResolver,
        _header_dep_resolver: &dyn HeaderDepResolver,
        _tx_dep_provider: &dyn TransactionDependencyProvider,
    ) -> Result<TransactionView, TxBuilderError> {
        let sender_query = {
            let mut query = CellQueryOptions::new_lock(self.sender.clone());
            query.secondary_script = Some(self.type_script.clone());
            query.data_len_range = Some(ValueRangeOption::new_min(16));
            query
        };
        let (sender_cells, _) = cell_collector.collect_live_cells(&sender_query, true)?;
        if sender_cells.is_empty() {
            return Err(TxBuilderError::Other(
                "sender cell not found".to_string().into(),
            ));
        }
        let sender_cell = &sender_cells[0];

        let sender_script_id = ScriptId::from(&self.sender);
        let sender_cell_dep = cell_dep_resolver
            .resolve(&sender_script_id)
            .ok_or(TxBuilderError::ResolveCellDepFailed(sender_script_id))?;
        let type_script_id = ScriptId::from(&self.type_script);
        let udt_cell_dep = cell_dep_resolver
            .resolve(&type_script_id)
            .ok_or(TxBuilderError::ResolveCellDepFailed(type_script_id))?;
        #[allow(clippy::mutable_key_type)]
        let mut cell_deps = HashSet::new();
        cell_deps.insert(sender_cell_dep);
        cell_deps.insert(udt_cell_dep);

        let mut amount_bytes = [0u8; 16];
        amount_bytes.copy_from_slice(&sender_cell.output_data.as_ref()[0..16]);
        let input_total = u128::from_le_bytes(amount_bytes);
        let output_total: u128 = self.receivers.iter().map(|receiver| receiver.amount).sum();
        if input_total < output_total {
            return Err(TxBuilderError::Other(
                format!(
                    "sender udt amount not enough, expected at least: {}, actual: {}",
                    output_total, input_total
                )
                .into(),
            ));
        }

        let sender_output_data = {
            let new_amount = input_total - output_total;
            let mut new_data = sender_cell.output_data.as_ref().to_vec();
            new_data[0..16].copy_from_slice(&new_amount.to_le_bytes()[..]);
            Bytes::from(new_data)
        };

        let mut inputs = vec![CellInput::new(sender_cell.out_point.clone(), 0)];
        let mut outputs = vec![sender_cell.output.clone()];
        let mut outputs_data = vec![sender_output_data.pack()];

        for receiver in &self.receivers {
            let ReceiverBuildOutput {
                input,
                cell_dep,
                output,
                output_data,
            } = receiver.build(&self.type_script, cell_collector, cell_dep_resolver)?;
            if let Some(input) = input {
                inputs.push(input);
            }
            if let Some(cell_dep) = cell_dep {
                cell_deps.insert(cell_dep);
            }
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
