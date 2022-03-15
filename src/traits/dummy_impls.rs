use ckb_chain_spec::consensus::Consensus;
use ckb_types::{
    bytes::Bytes,
    core::{HeaderView, TransactionView},
    packed::{Byte32, CellOutput, OutPoint, Transaction},
};

use crate::traits::{
    CellCollector, CellCollectorError, CellQueryOptions, HeaderDepResolver, LiveCell,
    TransactionDependencyError, TransactionDependencyProvider,
};

/// A dummy CellCollector. All methods will return error if possible.
#[derive(Default)]
pub struct DummyCellCollector;

impl CellCollector for DummyCellCollector {
    fn collect_live_cells(
        &mut self,
        _query: &CellQueryOptions,
        _apply_changes: bool,
    ) -> Result<(Vec<LiveCell>, u64), CellCollectorError> {
        Err(CellCollectorError::Other(
            "dummy collect_live_cells".to_string().into(),
        ))
    }

    fn lock_cell(&mut self, _out_point: OutPoint) -> Result<(), CellCollectorError> {
        Err(CellCollectorError::Other(
            "dummy lock_cell".to_string().into(),
        ))
    }

    fn apply_tx(&mut self, _tx: Transaction) -> Result<(), CellCollectorError> {
        Err(CellCollectorError::Other(
            "dummy apply_tx".to_string().into(),
        ))
    }
    fn reset(&mut self) {}
}

/// A dummy HeaderDepResolver. All methods will return error if possible.
#[derive(Default)]
pub struct DummyHeaderDepResolver;

impl HeaderDepResolver for DummyHeaderDepResolver {
    fn resolve_by_tx(
        &self,
        _tx_hash: &Byte32,
    ) -> Result<Option<HeaderView>, Box<dyn std::error::Error>> {
        Err("dummy resolve_by_tx".to_string().into())
    }
    fn resolve_by_number(
        &self,
        _number: u64,
    ) -> Result<Option<HeaderView>, Box<dyn std::error::Error>> {
        Err("dummy resolve_by_number".to_string().into())
    }
}

/// A dummy HeaderDepResolver. All methods will return error if possible.
#[derive(Default)]
pub struct DummyTransactionDependencyProvider;

impl TransactionDependencyProvider for DummyTransactionDependencyProvider {
    fn get_consensus(&self) -> Result<Consensus, TransactionDependencyError> {
        Err(TransactionDependencyError::Other(
            "dummy get_consensus".to_string().into(),
        ))
    }
    // For verify certain cell belong to certain transaction
    fn get_transaction(
        &self,
        _tx_hash: &Byte32,
    ) -> Result<TransactionView, TransactionDependencyError> {
        Err(TransactionDependencyError::Other(
            "dummy get_transaction".to_string().into(),
        ))
    }
    // For get the output information of inputs or cell_deps, those cell should be live cell
    fn get_cell(&self, _out_point: &OutPoint) -> Result<CellOutput, TransactionDependencyError> {
        Err(TransactionDependencyError::Other(
            "dummy get_cell".to_string().into(),
        ))
    }
    // For get the output data information of inputs or cell_deps
    fn get_cell_data(&self, _out_point: &OutPoint) -> Result<Bytes, TransactionDependencyError> {
        Err(TransactionDependencyError::Other(
            "dummy get_cell_data".to_string().into(),
        ))
    }
    // For get the header information of header_deps
    fn get_header(&self, _block_hash: &Byte32) -> Result<HeaderView, TransactionDependencyError> {
        Err(TransactionDependencyError::Other(
            "dummy get_header".to_string().into(),
        ))
    }
}
