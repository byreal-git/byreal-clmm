use crate::error::ErrorCode;
use crate::states::*;
use anchor_lang::prelude::*;
use anchor_spl::token_interface::{self, Mint, TokenAccount, TokenInterface};
use std::ops::DerefMut;

#[derive(Accounts)]
pub struct WithdrawOffchainRewardAccounts<'info> {
    /// The authority make decision that who can withdraw the offchain reward.
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

    /// the address who receive the withdrawn offchain reward.
    #[account(
        mut,
        token::mint = token_mint,
        token::token_program = token_program,
    )]
    pub receiver_token_account: Box<InterfaceAccount<'info, TokenAccount>>,

    #[account(
        mut,
        associated_token::mint = token_mint,
        associated_token::authority = reward_config,
        associated_token::token_program = token_program,
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
}

/// withdraw offchain reward into the reward vault.
pub fn withdraw_offchain_reward(ctx: Context<WithdrawOffchainRewardAccounts>, amount: u64) -> Result<()> {
    require_keys_eq!(
        *ctx.accounts.reward_config.to_account_info().owner,
        crate::id(),
        ErrorCode::IllegalAccountOwner
    );

    let reward_config_account_info = ctx.accounts.reward_config.to_account_info();
    let reward_config = ctx.accounts.reward_config.deref_mut();

    require_keys_eq!(
        reward_config.reward_vault,
        reward_config.key(),
        ErrorCode::InvalidAccount
    );

    if !reward_config.reward_mint_vec.contains(&ctx.accounts.token_mint.key()) {
        return err!(ErrorCode::NotSupportMint);
    }

    // make sure amount is enough
    let amount = if amount >= ctx.accounts.reward_vault_token_account.amount {
        // if we withdraw all the remaining amount, we also remove the mint from the config
        reward_config.remove_reward_mint(ctx.accounts.token_mint.key())?;
        // if the amount is larger than the vault, we withdraw all the remaining amount
        ctx.accounts.reward_vault_token_account.amount
    } else {
        amount
    };
    require_gt!(amount, 0);

    // transfer the token to the claimer's token account
    let decimals = ctx.accounts.token_mint.decimals;
    let seeds = reward_config.seeds();

    let cpi_accounts = token_interface::TransferChecked {
        to: ctx.accounts.receiver_token_account.to_account_info(),
        from: ctx.accounts.reward_vault_token_account.to_account_info(),
        mint: ctx.accounts.token_mint.to_account_info(),
        authority: reward_config_account_info,
    };

    token_interface::transfer_checked(
        CpiContext::new_with_signer(ctx.accounts.token_program.to_account_info(), cpi_accounts, &[&seeds]),
        amount,
        decimals,
    )?;

    Ok(())
}
