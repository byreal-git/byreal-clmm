use crate::error::ErrorCode;
use crate::states::*;
use anchor_lang::prelude::*;
use std::ops::DerefMut;

#[derive(Accounts)]
#[instruction(index: u16)]
pub struct UpdateAdminGroupAccounts<'info> {
    /// only super admin can create admin group
    #[account(
        mut,
        address = crate::admin::ID @ ErrorCode::NotApproved
    )]
    pub payer: Signer<'info>,

    /// update amm admin group account to store admin permissions.
    #[account(
        mut,
        seeds = [
            ADMIN_GROUP_SEED.as_bytes()
        ],
        bump,
    )]
    pub admin_group: Account<'info, AmmAdminGroup>,
}

#[derive(Clone, AnchorSerialize, AnchorDeserialize, Debug)]
pub struct UpdateAdminGroupParams {
    /// the address who can hold the fee,
    /// anyone can trigger the fee collection action,
    pub fee_keeper: Option<Pubkey>,

    /// the address who can config the reward(config, deposit, withdraw),
    /// deposit reward, set the account who can deposit the reward, withdraw the remaining reward(withdraw)
    pub reward_config_manager: Option<Pubkey>,

    /// the address who can manage the offchain reward claim
    pub reward_claim_manager: Option<Pubkey>,

    /// the address who can manage the pool create action,
    /// without this account's permission, no one can create a pool
    pub pool_manager: Option<Pubkey>,

    /// the address who can manage the emergency action,
    /// emergency action includes stop/resume the pool, stop/resume withdraw lp
    pub emergency_manager: Option<Pubkey>,

    /// normal action manager,
    /// such as create amm config, update amm config
    pub normal_manager: Option<Pubkey>,
}

pub fn update_amm_admin_group(ctx: Context<UpdateAdminGroupAccounts>, params: UpdateAdminGroupParams) -> Result<()> {
    let admin_group = ctx.accounts.admin_group.deref_mut();

    if let Some(fee_keeper) = params.fee_keeper {
        admin_group.fee_keeper = fee_keeper;
    }
    if let Some(reward_manager) = params.reward_config_manager {
        admin_group.reward_config_manager = reward_manager;
    }
    if let Some(reward_claim_manager) = params.reward_claim_manager {
        admin_group.reward_claim_manager = reward_claim_manager;
    }
    if let Some(pool_manager) = params.pool_manager {
        admin_group.pool_manager = pool_manager;
    }
    if let Some(emergency_manager) = params.emergency_manager {
        admin_group.emergency_manager = emergency_manager;
    }
    if let Some(normal_manager) = params.normal_manager {
        admin_group.normal_manager = normal_manager;
    }

    admin_group.validate()?;

    emit!(ModifyAmmAdminGroupEvent {
        fee_keeper: admin_group.fee_keeper,
        reward_config_manager: admin_group.reward_config_manager,
        reward_claim_manager: admin_group.reward_claim_manager,
        pool_manager: admin_group.pool_manager,
        emergency_manager: admin_group.emergency_manager,
        normal_manager: admin_group.normal_manager,
    });

    Ok(())
}
