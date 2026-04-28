use crate::error::ErrorCode;
use anchor_lang::prelude::*;

pub const ADMIN_GROUP_SEED: &str = "admin_group";

/// Holds the admin group information.
#[account]
#[derive(Default, Debug, InitSpace)]
pub struct AmmAdminGroup {
    /// the address who can hold the fee,
    /// anyone can trigger the fee collection action,
    pub fee_keeper: Pubkey,

    /// the address who can config the reward(config, deposit, withdraw),
    /// deposit reward, set the account who can deposit the reward, withdraw the remaining reward(withdraw)
    pub reward_config_manager: Pubkey,

    /// the address who can manage the offchain reward claim
    pub reward_claim_manager: Pubkey,

    /// the address who can manage the pool create action,
    /// without this account's permission, no one can create a pool
    pub pool_manager: Pubkey,

    /// the address who can manage the emergency action,
    /// emergency action includes stop/resume the pool, stop/resume withdraw lp
    pub emergency_manager: Pubkey,

    /// normal action manager,
    /// such as create amm config, update amm config
    pub normal_manager: Pubkey,

    /// The space required for the account. may be used for future extensions.
    pub pad: [Pubkey; 6],
}

impl AmmAdminGroup {
    pub const LEN: usize = 8 + Self::INIT_SPACE;

    pub fn validate(&self) -> Result<()> {
        require!(self.fee_keeper != Pubkey::default(), ErrorCode::NotApproved);
        require!(self.reward_config_manager != Pubkey::default(), ErrorCode::NotApproved);
        require!(self.reward_claim_manager != Pubkey::default(), ErrorCode::NotApproved);
        require!(self.pool_manager != Pubkey::default(), ErrorCode::NotApproved);
        require!(self.emergency_manager != Pubkey::default(), ErrorCode::NotApproved);
        require!(self.normal_manager != Pubkey::default(), ErrorCode::NotApproved);

        Ok(())
    }
}

#[event]
pub struct ModifyAmmAdminGroupEvent {
    pub fee_keeper: Pubkey,
    pub reward_config_manager: Pubkey,
    pub reward_claim_manager: Pubkey,
    pub pool_manager: Pubkey,
    pub emergency_manager: Pubkey,
    pub normal_manager: Pubkey,
}
