use crate::error::ErrorCode;
use crate::states::*;
use anchor_lang::prelude::*;
use anchor_spl::token_interface::{self, Mint, TokenAccount, TokenInterface};
use std::ops::Deref;

#[derive(Accounts)]
pub struct ClaimOffchainRewardAccounts<'info> {
    /// the address who claim the offchain reward.
    #[account(mut)]
    pub claimer: Signer<'info>,

    /// The authority make decision that who can claim the offchain reward.
    #[account(
        address = admin_group.reward_claim_manager @ ErrorCode::NotApproved
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
        token::authority = claimer,
        token::token_program = token_program,
    )]
    pub claimer_token_account: Box<InterfaceAccount<'info, TokenAccount>>,

    #[account(
        mut,
        associated_token::mint = token_mint,
        associated_token::authority = reward_config,
        associated_token::token_program = token_program,
    )]
    pub reward_vault_token_account: Box<InterfaceAccount<'info, TokenAccount>>,

    /// The offchain reward config account, it also is the reward vault account.
    #[account(
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
}

/// Claim offchain reward into the reward vault.
pub fn claim_offchain_reward(ctx: Context<ClaimOffchainRewardAccounts>, amount: u64) -> Result<()> {
    require_keys_eq!(
        *ctx.accounts.reward_config.to_account_info().owner,
        crate::id(),
        ErrorCode::IllegalAccountOwner
    );

    let reward_config = ctx.accounts.reward_config.deref();

    require_keys_eq!(
        reward_config.reward_vault,
        reward_config.key(),
        ErrorCode::InvalidAccount
    );

    if !reward_config.reward_mint_vec.contains(&ctx.accounts.token_mint.key()) {
        return err!(ErrorCode::NotSupportMint);
    }

    // transfer the token to the claimer's token account
    let decimals = ctx.accounts.token_mint.decimals;
    let seeds = reward_config.seeds();

    let cpi_accounts = token_interface::TransferChecked {
        to: ctx.accounts.claimer_token_account.to_account_info(),
        from: ctx.accounts.reward_vault_token_account.to_account_info(),
        mint: ctx.accounts.token_mint.to_account_info(),
        authority: ctx.accounts.reward_config.to_account_info(),
    };

    token_interface::transfer_checked(
        CpiContext::new_with_signer(ctx.accounts.token_program.to_account_info(), cpi_accounts, &[&seeds]),
        amount,
        decimals,
    )?;

    Ok(())
}
