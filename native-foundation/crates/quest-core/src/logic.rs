//! Gate semantics adapted from BetterQuesting EnumLogic (MIT).
//! Copyright (c) 2019 Funwayguy. See ../../../licenses/BetterQuesting-MIT.txt.
use crate::Error;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Logic {
    AND,
    NAND,
    OR,
    NOR,
    XOR,
    XNOR,
}
impl Logic {
    pub fn evaluate(self, complete: u32, total: u32) -> Result<bool, Error> {
        if complete > total {
            return Err("completed input count exceeds total".into());
        }
        Ok(match self {
            Self::AND => complete == total,
            Self::NAND => complete < total,
            Self::OR => complete > 0,
            Self::NOR => complete == 0,
            Self::XOR => complete == 1,
            Self::XNOR => total.checked_sub(1) == Some(complete),
        })
    }
    pub fn unlockable(self, complete: u32, total: u32) -> Result<bool, Error> {
        if complete > total {
            return Err("completed input count exceeds total".into());
        }
        Ok(match self {
            Self::AND | Self::OR => true,
            Self::NAND | Self::XNOR => complete < total,
            Self::NOR => complete == 0,
            Self::XOR => complete <= 1,
        })
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn all_six_gates_match_the_pinned_reference_truth_table() {
        for (gate, table) in [
            (Logic::AND, [false, false, false, false, true]),
            (Logic::NAND, [true, true, true, true, false]),
            (Logic::OR, [false, true, true, true, true]),
            (Logic::NOR, [true, false, false, false, false]),
            (Logic::XOR, [false, true, false, false, false]),
            (Logic::XNOR, [false, false, false, true, false]),
        ] {
            for (count, expected) in table.into_iter().enumerate() {
                assert_eq!(
                    gate.evaluate(count as u32, 4).unwrap(),
                    expected,
                    "{gate:?} with {count}"
                );
            }
            assert!(gate.evaluate(5, 4).is_err());
            assert!(gate.unlockable(5, 4).is_err());
        }
    }
    #[test]
    fn zero_inputs_and_exactly_one_xor_are_explicit() {
        assert!(Logic::AND.evaluate(0, 0).unwrap());
        assert!(Logic::NOR.evaluate(0, 0).unwrap());
        assert!(!Logic::XNOR.evaluate(0, 0).unwrap());
        assert!(!Logic::XOR.evaluate(3, 4).unwrap());
        assert!(Logic::AND.unlockable(4, 4).unwrap());
        assert!(Logic::OR.unlockable(4, 4).unwrap());
        assert!(!Logic::XOR.unlockable(2, 4).unwrap());
        assert!(Logic::XOR.unlockable(1, 4).unwrap());
        assert!(!Logic::XNOR.unlockable(4, 4).unwrap());
        assert!(Logic::XNOR.unlockable(3, 4).unwrap());
    }
}
