use std::ops::DerefMut;

use crate::error::ErrorCode;
use crate::states::*;
use anchor_lang::prelude::*;
use anchor_spl::token_interface::{self, Mint, TokenAccount, TokenInterface};

#[derive(Accounts)]
pub struct DepositOffchainRewardAccounts<'info> {
    /// the address paying to deposit the offchain reward.
    #[account(mut)]
    pub payer: Signer<'info>,

    /// The authority make decision that who can deposit the offchain reward.
    #[account(
        address = admin_group.reward_config_manager @ ErrorCode::NotApproved
    )]
    pub authority: Signer<'info>,

    /// Initialize amm admin group account to store admin permissions.
    #[account(
        seeds = [
            ADMIN_GROUP_SEED.as_bytes()
        ],
        bump,
    )]
    pub admin_group: Account<'info, AmmAdminGroup>,

    /// the pool id, which is the pool state account.
    /// CHECK: only used to derive the reward config account.
    pub pool_id: UncheckedAccount<'info>,

    #[account(
        mint::token_program = token_program
    )]
    pub token_mint: Box<InterfaceAccount<'info, Mint>>,

    ///
    #[account(
        mut,
        token::mint = token_mint,
        token::authority = payer,
        token::token_program = token_program,
    )]
    pub payer_token_account: Box<InterfaceAccount<'info, TokenAccount>>,

    #[account(
        init_if_needed,
        associated_token::mint = token_mint,
        associated_token::authority = reward_config,
        associated_token::token_program = token_program,
        payer = payer,
    )]
    pub reward_vault_token_account: Box<InterfaceAccount<'info, TokenAccount>>,

    /// The offchain reward config account, it also is the reward vault account.
    #[account(
        mut,
        seeds = [
            OFFCHAIN_REWARD_SEED.as_bytes(),
            pool_id.key().as_ref(),
        ],
        bump,
        has_one = pool_id
    )]
    pub reward_config: Box<Account<'info, OffchainRewardConfig>>,

    /// Spl token program or token program 2022
    pub token_program: Interface<'info, TokenInterface>,

    pub associated_token_program: Program<'info, anchor_spl::associated_token::AssociatedToken>,

    pub system_program: Program<'info, System>,
}

/// Deposit offchain reward into the reward vault.
pub fn deposit_offchain_reward(ctx: Context<DepositOffchainRewardAccounts>, amount: u64) -> Result<()> {
    let reward_config = ctx.accounts.reward_config.deref_mut();

    require_keys_eq!(
        reward_config.reward_vault,
        reward_config.key(),
        ErrorCode::InvalidAccount
    );

    // add reward mint to the config if not exists, before that, check if the config account has enough space
    if reward_config.add_reward_mint(ctx.accounts.token_mint.key())? {
        // reallocate the account if needed
        OffchainRewardConfig::realloc_if_needed(
            reward_config.to_account_info(),
            reward_config.reward_mint_vec.len(),
            ctx.accounts.payer.to_account_info(),
            ctx.accounts.system_program.to_account_info(),
        )?;
    }

    // transfer the token to the reward vault
    let decimals = ctx.accounts.token_mint.decimals;

    let cpi_accounts = token_interface::TransferChecked {
        from: ctx.accounts.payer_token_account.to_account_info(),
        to: ctx.accounts.reward_vault_token_account.to_account_info(),
        mint: ctx.accounts.token_mint.to_account_info(),
        authority: ctx.accounts.payer.to_account_info(),
    };
    token_interface::transfer_checked(
        CpiContext::new(ctx.accounts.token_program.to_account_info(), cpi_accounts),
        amount,
        decimals,
    )?;

    Ok(())
}
