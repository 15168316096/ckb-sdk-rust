use std::collections::HashMap;

use ckb_script::ScriptGroup;
use ckb_types::{
    bytes::Bytes,
    core::TransactionView,
    packed::{Byte32, WitnessArgs},
    prelude::*,
};
use thiserror::Error;

use super::signer::{
    AnyoneCanPaySigner, ChequeSigner, ScriptSignError, ScriptSigner, Secp256k1MultisigSigner,
    Secp256k1SighashSigner,
};
use crate::traits::{TransactionDependencyError, TransactionDependencyProvider};
use crate::types::ScriptId;

const CHEQUE_CLAIM_SINCE: u64 = 0;
const CHEQUE_WITHDRAW_SINCE: u64 = 0xA000000000000006;

#[derive(Error, Debug)]
pub enum UnlockError {
    #[error("sign script error: `{0}`")]
    ScriptSigner(#[from] ScriptSignError),
    #[error("transaction dependency error: `{0}`")]
    TxDep(#[from] TransactionDependencyError),
    #[error("other error: `{0}`")]
    Other(#[from] Box<dyn std::error::Error>),
}

/// Script unlock logic:
///   * Parse the script.args
///   * Sign the transaction
///   * Put extra unlock information into transaction (e.g. SMT proof in omni-lock case)
pub trait ScriptUnlocker {
    fn match_args(&self, args: &[u8]) -> bool;

    fn is_unlocked(
        &self,
        _tx: &TransactionView,
        _script_group: &ScriptGroup,
        _tx_dep_provider: &dyn TransactionDependencyProvider,
    ) -> Result<bool, UnlockError> {
        Ok(false)
    }
    // Add signature or other information to witnesses
    fn unlock(
        &self,
        tx: &TransactionView,
        script_group: &ScriptGroup,
        tx_dep_provider: &dyn TransactionDependencyProvider,
    ) -> Result<TransactionView, UnlockError>;
}

#[derive(Default)]
pub struct ScriptUnlockerManager {
    items: HashMap<ScriptId, Box<dyn ScriptUnlocker>>,
}

impl ScriptUnlockerManager {
    pub fn register(&mut self, script_id: ScriptId, unlocker: Box<dyn ScriptUnlocker>) {
        self.items.insert(script_id, unlocker);
    }

    pub fn get_mut(&mut self, script_id: &ScriptId) -> Option<&mut Box<dyn ScriptUnlocker>> {
        self.items.get_mut(script_id)
    }
}

pub struct Secp256k1SighashUnlocker {
    signer: Secp256k1SighashSigner,
}
impl Secp256k1SighashUnlocker {
    pub fn new(signer: Secp256k1SighashSigner) -> Secp256k1SighashUnlocker {
        Secp256k1SighashUnlocker { signer }
    }
}
impl ScriptUnlocker for Secp256k1SighashUnlocker {
    fn match_args(&self, args: &[u8]) -> bool {
        self.signer.match_args(args)
    }

    fn unlock(
        &self,
        tx: &TransactionView,
        script_group: &ScriptGroup,
        _tx_dep_provider: &dyn TransactionDependencyProvider,
    ) -> Result<TransactionView, UnlockError> {
        Ok(self.signer.sign_tx(tx, script_group)?)
    }
}

pub struct Secp256k1MultisigUnlocker {
    signer: Secp256k1MultisigSigner,
}
impl Secp256k1MultisigUnlocker {
    pub fn new(signer: Secp256k1MultisigSigner) -> Secp256k1MultisigUnlocker {
        Secp256k1MultisigUnlocker { signer }
    }
}
impl ScriptUnlocker for Secp256k1MultisigUnlocker {
    fn match_args(&self, args: &[u8]) -> bool {
        (args.len() == 20 || args.len() == 28) && self.signer.match_args(args)
    }

    fn unlock(
        &self,
        tx: &TransactionView,
        script_group: &ScriptGroup,
        _tx_dep_provider: &dyn TransactionDependencyProvider,
    ) -> Result<TransactionView, UnlockError> {
        Ok(self.signer.sign_tx(tx, script_group)?)
    }
}

pub struct AnyoneCanPayUnlocker {
    signer: AnyoneCanPaySigner,
}

impl AnyoneCanPayUnlocker {
    pub fn new(signer: AnyoneCanPaySigner) -> AnyoneCanPayUnlocker {
        AnyoneCanPayUnlocker { signer }
    }
}
impl ScriptUnlocker for AnyoneCanPayUnlocker {
    fn match_args(&self, args: &[u8]) -> bool {
        self.signer.match_args(args)
    }

    fn is_unlocked(
        &self,
        tx: &TransactionView,
        script_group: &ScriptGroup,
        tx_dep_provider: &dyn TransactionDependencyProvider,
    ) -> Result<bool, UnlockError> {
        const POW10: [u64; 20] = [
            1,
            10,
            100,
            1000,
            10000,
            100000,
            1000000,
            10000000,
            100000000,
            1000000000,
            10000000000,
            100000000000,
            1000000000000,
            10000000000000,
            100000000000000,
            1000000000000000,
            10000000000000000,
            100000000000000000,
            1000000000000000000,
            10000000000000000000,
        ];
        let script_args = script_group.script.args().raw_data();
        let min_ckb_amount = if script_args.len() > 20 {
            let idx = script_args.as_ref()[20];
            if idx >= 20 {
                return Err(UnlockError::Other(format!("invalid min ckb amount config in script.args, got: {}, expected: value >=0 and value < 20", idx).into()));
            }
            POW10[idx as usize]
        } else {
            0
        };
        let min_udt_amount = if script_args.len() > 21 {
            let idx = script_args.as_ref()[21];
            if idx >= 39 {
                return Err(UnlockError::Other(format!("invalid min udt amount config in script.args, got: {}, expected: value >=0 and value < 39", idx).into()));
            }
            if idx >= 20 {
                (POW10[19] as u128) * (POW10[idx as usize - 19] as u128)
            } else {
                POW10[idx as usize] as u128
            }
        } else {
            0
        };

        struct InputWallet {
            type_hash_opt: Option<Byte32>,
            ckb_amount: u64,
            udt_amount: u128,
            output_cnt: usize,
        }
        let mut input_wallets = script_group
            .input_indices
            .iter()
            .map(|idx| {
                let input = tx.inputs().get(*idx).ok_or_else(|| {
                    UnlockError::Other(
                        format!("input index in script group is out of bound: {}", idx).into(),
                    )
                })?;
                let output = tx_dep_provider.get_cell(&input.previous_output())?;
                let output_data = tx_dep_provider.get_cell_data(&input.previous_output())?;

                let type_hash_opt = output
                    .type_()
                    .to_opt()
                    .map(|script| script.calc_script_hash());
                if type_hash_opt.is_some() && output_data.len() < 16 {
                    return Err(UnlockError::Other(
                        format!("invalid udt output data in input cell: {:?}", input).into(),
                    ));
                }
                let udt_amount = if type_hash_opt.is_some() {
                    let mut amount_bytes = [0u8; 16];
                    amount_bytes.copy_from_slice(&output_data[0..16]);
                    u128::from_le_bytes(amount_bytes)
                } else {
                    0
                };
                Ok(InputWallet {
                    type_hash_opt,
                    ckb_amount: output.capacity().unpack(),
                    udt_amount,
                    output_cnt: 0,
                })
            })
            .collect::<Result<Vec<InputWallet>, UnlockError>>()?;

        for output_idx in &script_group.output_indices {
            let output = tx.output(*output_idx).ok_or_else(|| {
                UnlockError::Other(
                    format!(
                        "output index in script group is out of bound: {}",
                        output_idx
                    )
                    .into(),
                )
            })?;
            let output_data: Bytes = tx
                .outputs_data()
                .get(*output_idx)
                .map(|data| data.raw_data())
                .ok_or_else(|| {
                    UnlockError::Other(
                        format!(
                            "output data index in script group is out of bound: {}",
                            output_idx
                        )
                        .into(),
                    )
                })?;
            let type_hash_opt = output
                .type_()
                .to_opt()
                .map(|script| script.calc_script_hash());
            if type_hash_opt.is_some() && output_data.len() < 16 {
                return Err(UnlockError::Other(
                    format!(
                        "invalid udt output data in output cell: index={}",
                        output_idx
                    )
                    .into(),
                ));
            }
            let ckb_amount: u64 = output.capacity().unpack();
            let udt_amount = if type_hash_opt.is_some() {
                let mut amount_bytes = [0u8; 16];
                amount_bytes.copy_from_slice(&output_data[0..16]);
                u128::from_le_bytes(amount_bytes)
            } else {
                0
            };
            let mut found_inputs = 0;
            for input_wallet in &mut input_wallets {
                if input_wallet.type_hash_opt == type_hash_opt {
                    let (min_output_ckb_amount, ckb_overflow) =
                        input_wallet.ckb_amount.overflowing_add(min_ckb_amount);
                    let meet_ckb_cond = !ckb_overflow && ckb_amount >= min_output_ckb_amount;
                    let (min_output_udt_amount, udt_overflow) =
                        input_wallet.udt_amount.overflowing_add(min_udt_amount);
                    let meet_udt_cond = !udt_overflow && udt_amount >= min_output_udt_amount;
                    if !(meet_ckb_cond || meet_udt_cond) {
                        // ERROR_OUTPUT_AMOUNT_NOT_ENOUGH
                        return Ok(false);
                    }
                    if (!meet_ckb_cond && ckb_amount != input_wallet.ckb_amount)
                        || (!meet_udt_cond && udt_amount != input_wallet.udt_amount)
                    {
                        // ERROR_OUTPUT_AMOUNT_NOT_ENOUGH
                        return Ok(false);
                    }
                    found_inputs += 1;
                    input_wallet.output_cnt += 1;
                    if found_inputs > 1 {
                        // ERROR_DUPLICATED_INPUTS
                        return Ok(false);
                    }
                    if input_wallet.output_cnt > 1 {
                        // ERROR_DUPLICATED_OUTPUTS
                        return Ok(false);
                    }
                }
            }
            if found_inputs != 1 {
                // ERROR_NO_PAIR + ERROR_DUPLICATED_INPUTS
                return Ok(false);
            }
        }
        for input_wallet in &input_wallets {
            if input_wallet.output_cnt != 1 {
                // ERROR_NO_PAIR + ERROR_DUPLICATED_OUTPUTS
                return Ok(false);
            }
        }
        Ok(true)
    }
    fn unlock(
        &self,
        tx: &TransactionView,
        script_group: &ScriptGroup,
        tx_dep_provider: &dyn TransactionDependencyProvider,
    ) -> Result<TransactionView, UnlockError> {
        if self.is_unlocked(tx, script_group, tx_dep_provider)? {
            Ok(tx.clone())
        } else {
            Ok(self.signer.sign_tx(tx, script_group)?)
        }
    }
}

pub struct ChequeUnlocker {
    signer: ChequeSigner,
}
impl ChequeUnlocker {
    pub fn new(signer: ChequeSigner) -> ChequeUnlocker {
        ChequeUnlocker { signer }
    }
}

impl ScriptUnlocker for ChequeUnlocker {
    fn match_args(&self, args: &[u8]) -> bool {
        self.signer.match_args(args)
    }

    fn is_unlocked(
        &self,
        tx: &TransactionView,
        script_group: &ScriptGroup,
        tx_dep_provider: &dyn TransactionDependencyProvider,
    ) -> Result<bool, UnlockError> {
        let args = script_group.script.args().raw_data();
        if args.len() != 40 {
            return Err(UnlockError::Other(
                format!(
                    "invalid script args length, expected: 40, got: {}",
                    args.len()
                )
                .into(),
            ));
        }
        let inputs: Vec<_> = tx.inputs().into_iter().collect();
        let group_since_list: Vec<u64> = script_group
            .input_indices
            .iter()
            .map(|idx| inputs[*idx].since().unpack())
            .collect();

        // Check if unlocked via lock hash in inputs
        let receiver_lock_hash = &args.as_ref()[0..20];
        let sender_lock_hash = &args.as_ref()[20..40];
        let mut receiver_lock_witness = None;
        let mut sender_lock_witness = None;
        for (input_idx, input) in inputs.into_iter().enumerate() {
            let output = tx_dep_provider.get_cell(&input.previous_output())?;
            let lock_hash = output.lock().calc_script_hash();
            let lock_hash_prefix = &lock_hash.as_slice()[0..20];
            let witness = tx
                .witnesses()
                .get(input_idx)
                .map(|witness| witness.raw_data())
                .unwrap_or_default();

            #[allow(clippy::collapsible_if)]
            if lock_hash_prefix == receiver_lock_hash {
                if receiver_lock_witness.is_none() {
                    receiver_lock_witness = Some((input_idx, witness));
                }
            } else if lock_hash_prefix == sender_lock_hash {
                if sender_lock_witness.is_none() {
                    sender_lock_witness = Some((input_idx, witness));
                }
            }
        }
        // NOTE: receiver has higher priority than sender
        if let Some((input_idx, witness)) = receiver_lock_witness {
            if group_since_list
                .iter()
                .any(|since| *since != CHEQUE_CLAIM_SINCE)
            {
                return Err(UnlockError::Other(
                    "claim action must have all zero since in cheque inputs"
                        .to_string()
                        .into(),
                ));
            }
            let witness_args = WitnessArgs::from_slice(witness.as_ref()).map_err(|_| {
                UnlockError::Other(
                    format!(
                        "malformed witness for receiver lock, input index: {}",
                        input_idx
                    )
                    .into(),
                )
            })?;
            if witness_args.lock().to_opt().is_none() {
                return Err(UnlockError::Other(
                    format!(
                        "witness has not lock field for receiver lock, input index: {}",
                        input_idx
                    )
                    .into(),
                ));
            }
            return Ok(true);
        } else if let Some((input_idx, witness)) = sender_lock_witness {
            if group_since_list
                .iter()
                .any(|since| *since != CHEQUE_WITHDRAW_SINCE)
            {
                return Err(UnlockError::Other(
                    "claim action must have all relative 6 epochs since in cheque inputs"
                        .to_string()
                        .into(),
                ));
            }
            let witness_args = WitnessArgs::from_slice(witness.as_ref()).map_err(|_| {
                UnlockError::Other(
                    format!(
                        "malformed witness for sender lock, input index: {}",
                        input_idx
                    )
                    .into(),
                )
            })?;
            if witness_args.lock().to_opt().is_none() {
                return Err(UnlockError::Other(
                    format!(
                        "witness has not lock field for sender lock, input index: {}",
                        input_idx
                    )
                    .into(),
                ));
            }
            return Ok(true);
        }
        Ok(false)
    }

    fn unlock(
        &self,
        tx: &TransactionView,
        script_group: &ScriptGroup,
        tx_dep_provider: &dyn TransactionDependencyProvider,
    ) -> Result<TransactionView, UnlockError> {
        if self.is_unlocked(tx, script_group, tx_dep_provider)? {
            Ok(tx.clone())
        } else {
            Ok(self.signer.sign_tx(tx, script_group)?)
        }
    }
}
