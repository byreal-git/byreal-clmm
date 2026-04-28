use crate::error::ErrorCode;
use crate::states::*;
use anchor_lang::prelude::*;
use std::ops::DerefMut;

#[derive(Accounts)]
#[instruction(index: u16)]
pub struct InitAdminGroupAccounts<'info> {
    /// only super admin can create admin group
    #[account(
        mut,
        address = crate::admin::ID @ ErrorCode::NotApproved
    )]
    pub payer: Signer<'info>,

    /// Initialize amm admin group account to store admin permissions.
    #[account(
        init,
        seeds = [
            ADMIN_GROUP_SEED.as_bytes()
        ],
        bump,
        payer = payer,
        space = AmmAdminGroup::LEN
    )]
    pub admin_group: Account<'info, AmmAdminGroup>,

    pub system_program: Program<'info, System>,
}

#[derive(Clone, AnchorSerialize, AnchorDeserialize, Debug)]
pub struct InitAdminGroupParams {
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
}

pub fn init_amm_admin_group(ctx: Context<InitAdminGroupAccounts>, params: InitAdminGroupParams) -> Result<()> {
    let admin_group = ctx.accounts.admin_group.deref_mut();

    admin_group.fee_keeper = params.fee_keeper;
    admin_group.reward_config_manager = params.reward_config_manager;
    admin_group.reward_claim_manager = params.reward_claim_manager;
    admin_group.pool_manager = params.pool_manager;
    admin_group.emergency_manager = params.emergency_manager;
    admin_group.normal_manager = params.normal_manager;

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
