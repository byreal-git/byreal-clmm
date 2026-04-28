use crate::error::ErrorCode;
use crate::states::*;
use crate::util::create_token_vault_account;
use crate::{libraries::tick_math, util};
use anchor_lang::{prelude::*, solana_program};
use anchor_spl::token_interface::{Mint, TokenInterface};
use std::ops::DerefMut;
// use solana_program::{program::invoke_signed, system_instruction};
#[derive(Accounts)]
pub struct CreatePool<'info> {
    /// Address paying to create the pool. Can be anyone
    #[account(mut)]
    pub pool_creator: Signer<'info>,

    /// with pool_manager permission, the pool creator can create a pool.
    #[account(address = admin_group.pool_manager @ ErrorCode::NotApproved,)]
    pub pool_manager: Signer<'info>,

    /// amm admin group account to store admin permissions.
    #[account(
        seeds = [
            ADMIN_GROUP_SEED.as_bytes()
        ],
        bump,
    )]
    pub admin_group: Box<Account<'info, AmmAdminGroup>>,

    /// Which config the pool belongs to.
    pub amm_config: Box<Account<'info, AmmConfig>>,

    /// Initialize an account to store the pool state
    #[account(
        init,
        seeds = [
            POOL_SEED.as_bytes(),
            amm_config.key().as_ref(),
            token_mint_0.key().as_ref(),
            token_mint_1.key().as_ref(),
        ],
        bump,
        payer = pool_creator,
        space = PoolState::LEN
    )]
    pub pool_state: AccountLoader<'info, PoolState>,

    /// Initialize an account to store the off-chain reward config
    #[account(
        init,
        seeds = [
            OFFCHAIN_REWARD_SEED.as_bytes(),
            pool_state.key().as_ref(),
        ],
        bump,
        payer = pool_creator,
        space = OffchainRewardConfig::need_len(0)
    )]
    pub offchain_reward_config: Box<Account<'info, OffchainRewardConfig>>,

    /// Token_0 mint, the key must be smaller then token_1 mint.
    #[account(
        constraint = token_mint_0.key() < token_mint_1.key(),
        mint::token_program = token_program_0
    )]
    pub token_mint_0: Box<InterfaceAccount<'info, Mint>>,

    /// Token_1 mint
    #[account(
        mint::token_program = token_program_1
    )]
    pub token_mint_1: Box<InterfaceAccount<'info, Mint>>,

    /// CHECK: Token_0 vault for the pool, initialized in contract
    #[account(
        mut,
        seeds =[
            POOL_VAULT_SEED.as_bytes(),
            pool_state.key().as_ref(),
            token_mint_0.key().as_ref(),
        ],
        bump,
    )]
    pub token_vault_0: UncheckedAccount<'info>,

    /// CHECK: Token_1 vault for the pool, initialized in contract
    #[account(
        mut,
        seeds =[
            POOL_VAULT_SEED.as_bytes(),
            pool_state.key().as_ref(),
            token_mint_1.key().as_ref(),
        ],
        bump,
    )]
    pub token_vault_1: UncheckedAccount<'info>,

    /// Initialize an account to store oracle observations
    #[account(
        init,
        seeds = [
            OBSERVATION_SEED.as_bytes(),
            pool_state.key().as_ref(),
        ],
        bump,
        payer = pool_creator,
        space = ObservationState::LEN
    )]
    pub observation_state: AccountLoader<'info, ObservationState>,

    /// Initialize an account to store if a tick array is initialized.
    #[account(
        init,
        seeds = [
            POOL_TICK_ARRAY_BITMAP_SEED.as_bytes(),
            pool_state.key().as_ref(),
        ],
        bump,
        payer = pool_creator,
        space = TickArrayBitmapExtension::LEN
    )]
    pub tick_array_bitmap: AccountLoader<'info, TickArrayBitmapExtension>,

    /// Spl token program or token program 2022
    pub token_program_0: Interface<'info, TokenInterface>,
    /// Spl token program or token program 2022
    pub token_program_1: Interface<'info, TokenInterface>,
    /// To create a new program account
    pub system_program: Program<'info, System>,
    /// Sysvar for program account
    pub rent: Sysvar<'info, Rent>,
    // remaining account
    // #[account(
    //     seeds = [
    //     SUPPORT_MINT_SEED.as_bytes(),
    //     token_mint_0.key().as_ref(),
    // ],
    //     bump
    // )]
    // pub support_mint0_associated: Account<'info, SupportMintAssociated>,

    // #[account(
    //     seeds = [
    //     SUPPORT_MINT_SEED.as_bytes(),
    //     token_mint_1.key().as_ref(),
    // ],
    //     bump
    // )]
    // pub support_mint1_associated: Account<'info, SupportMintAssociated>,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug, Default)]
pub struct CreatePoolDecayFeeParams {
    /// The initial sqrt price of the pool, in x64 format.
    pub sqrt_price_x64: u128,
    /// The open time of the pool, can be set to a future time; it should be None or future time.
    /// If None, the pool will be opened immediately.
    pub open_time: Option<u64>,

    /// Whether to use decay fee for the pool.
    /// If true, the pool will use decay fee.
    pub use_decay_fee: bool,

    /// Whether to use decay fee on sell for mint0 and mint1.
    /// If true, the pool will use decay fee on sell for mint0 and mint1
    /// these two fields are only used when use_decay_fee is true, and at least one of them must be true.
    pub decay_fee_on_sell_mint0: bool,
    pub decay_fee_on_sell_mint1: bool,

    /// The initial decay fee rate for the pool, in percentage.(1=1%)
    pub init_decay_fee_rate: u8,

    /// decrease rate for the decay fee, in percentage.(1=1%)
    pub decay_fee_decrease_rate: u8,

    /// The interval for decreasing the decay fee, in seconds.
    /// how to calculate the current decay fee:
    /// interval_count = (current_time - open_time) / decay_fee_decrease_interval
    /// current_decay_fee = init_decay_fee_rate*((1-decay_fee_decrease_rate/100)^interval_count)
    pub decay_fee_decrease_interval: u8,
}

pub fn create_pool_decay_fee(ctx: Context<CreatePool>, params: CreatePoolDecayFeeParams) -> Result<()> {
    let mint0_associated_is_initialized =
        util::support_mint_associated_is_initialized(&ctx.remaining_accounts, &ctx.accounts.token_mint_0)?;
    let mint1_associated_is_initialized =
        util::support_mint_associated_is_initialized(&ctx.remaining_accounts, &ctx.accounts.token_mint_1)?;
    if !(util::is_supported_mint(&ctx.accounts.token_mint_0, mint0_associated_is_initialized).unwrap()
        && util::is_supported_mint(&ctx.accounts.token_mint_1, mint1_associated_is_initialized).unwrap())
    {
        return err!(ErrorCode::NotSupportMint);
    }

    // we can set open-time as a future time
    let block_timestamp = solana_program::clock::Clock::get()?.unix_timestamp as u64;
    let open_time = params.open_time.unwrap_or(block_timestamp);
    require_gte!(open_time, block_timestamp);

    let pool_id = ctx.accounts.pool_state.key();

    // init offchain reward config
    {
        let reward_vault = ctx.accounts.offchain_reward_config.key();
        let vault_bump = ctx.bumps.offchain_reward_config;
        let offchain_reward_config = ctx.accounts.offchain_reward_config.deref_mut();

        offchain_reward_config.initialize(pool_id, reward_vault, vault_bump)?;
    }

    let mut pool_state = ctx.accounts.pool_state.load_init()?;

    let tick = tick_math::get_tick_at_sqrt_price(params.sqrt_price_x64)?;
    #[cfg(feature = "enable-log")]
    msg!(
        "create pool with decay-fee, init_price: {}, init_tick:{}",
        params.sqrt_price_x64,
        tick
    );

    // init token vault accounts
    create_token_vault_account(
        &ctx.accounts.pool_creator,
        &ctx.accounts.pool_state.to_account_info(),
        &ctx.accounts.token_vault_0,
        &ctx.accounts.token_mint_0,
        &ctx.accounts.system_program,
        &ctx.accounts.token_program_0,
        &[
            POOL_VAULT_SEED.as_bytes(),
            ctx.accounts.pool_state.key().as_ref(),
            ctx.accounts.token_mint_0.key().as_ref(),
            &[ctx.bumps.token_vault_0][..],
        ],
    )?;

    create_token_vault_account(
        &ctx.accounts.pool_creator,
        &ctx.accounts.pool_state.to_account_info(),
        &ctx.accounts.token_vault_1,
        &ctx.accounts.token_mint_1,
        &ctx.accounts.system_program,
        &ctx.accounts.token_program_1,
        &[
            POOL_VAULT_SEED.as_bytes(),
            ctx.accounts.pool_state.key().as_ref(),
            ctx.accounts.token_mint_1.key().as_ref(),
            &[ctx.bumps.token_vault_1][..],
        ],
    )?;

    // init observation
    ctx.accounts.observation_state.load_init()?.initialize(pool_id)?;

    let bump = ctx.bumps.pool_state;
    pool_state.initialize(
        bump,
        params.sqrt_price_x64,
        open_time,
        tick,
        ctx.accounts.pool_creator.key(),
        ctx.accounts.token_vault_0.key(),
        ctx.accounts.token_vault_1.key(),
        ctx.accounts.amm_config.as_ref(),
        ctx.accounts.token_mint_0.as_ref(),
        ctx.accounts.token_mint_1.as_ref(),
        ctx.accounts.observation_state.key(),
    )?;

    // if use decay fee, then config it
    if params.use_decay_fee {
        pool_state.initialize_decay_fee(
            params.decay_fee_on_sell_mint0,
            params.decay_fee_on_sell_mint1,
            params.init_decay_fee_rate,
            params.decay_fee_decrease_rate,
            params.decay_fee_decrease_interval,
        )?;
    }

    ctx.accounts.tick_array_bitmap.load_init()?.initialize(pool_id);

    emit!(PoolCreatedEvent {
        token_mint_0: ctx.accounts.token_mint_0.key(),
        token_mint_1: ctx.accounts.token_mint_1.key(),
        tick_spacing: ctx.accounts.amm_config.tick_spacing,
        pool_state: ctx.accounts.pool_state.key(),
        sqrt_price_x64: params.sqrt_price_x64,
        tick,
        token_vault_0: ctx.accounts.token_vault_0.key(),
        token_vault_1: ctx.accounts.token_vault_1.key(),
    });
    Ok(())
}
