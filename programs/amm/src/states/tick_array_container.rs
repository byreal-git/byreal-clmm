use anchor_lang::error::{Error, ErrorCode};
use anchor_lang::solana_program::account_info::AccountInfo;
use anchor_lang::{prelude::*, system_program};
use arrayref::array_ref;
use std::cell::RefMut;
use std::mem;
use std::ops::DerefMut;

use crate::error::ErrorCode as ClmmErrorCode;
use crate::states::{
    DynTickArrayLoader, DynTickArrayState, PoolState, TickArrayState, TickState, TickUtils, TICK_ARRAY_SEED,
};
use crate::util::*;

/// Unified TickArray account view container
#[derive(Clone)]
pub enum TickArrayContainer<'info> {
    Fixed(AccountLoad<'info, TickArrayState>),
    Dynamic(DynTickArrayLoader<'info>),
}

pub enum TickArrayContainerRefMut<'info> {
    Fixed(RefMut<'info, TickArrayState>),
    Dynamic((RefMut<'info, DynTickArrayState>, RefMut<'info, [TickState]>)),
}

impl TickArrayContainer<'_> {
    /// Get mutable reference to the underlying TickArrayState or (DynTickArrayState, [TickState])
    pub fn get_ref_mut(&self) -> Result<TickArrayContainerRefMut<'_>> {
        match self {
            TickArrayContainer::Fixed(loader) => {
                let tick_array = loader.load_mut()?;
                Ok(TickArrayContainerRefMut::Fixed(tick_array))
            }

            TickArrayContainer::Dynamic(dyn_loader) => {
                let (dyn_tick_header, dyn_tick_states) = dyn_loader.load_mut(false)?;
                Ok(TickArrayContainerRefMut::Dynamic((dyn_tick_header, dyn_tick_states)))
            }
        }
    }

    /// Returns a `RefMut` to the account data structure for reading or writing directly.
    /// There is no need to convert AccountInfo to AccountLoad. (will expand RefMut lifetime to 'a)
    /// So it is necessary to check the owner
    pub fn load_data_mut<'a>(acc_info: &'a AccountInfo) -> Result<TickArrayContainerRefMut<'a>> {
        if acc_info.owner != &crate::id() {
            return Err(Error::from(ErrorCode::AccountOwnedByWrongProgram).with_pubkeys((*acc_info.owner, crate::id())));
        }
        if !acc_info.is_writable {
            return Err(ErrorCode::AccountNotMutable.into());
        }

        let data = acc_info.try_borrow_mut_data()?;
        let data_len = data.len();
        if data_len < DynTickArrayState::DISCRIMINATOR.len() {
            return Err(ErrorCode::AccountDiscriminatorNotFound.into());
        }

        let disc_bytes = array_ref![data, 0, 8];

        if disc_bytes == DynTickArrayState::DISCRIMINATOR {
            let (header, ticks) = RefMut::map_split(data, |data_slice| {
                let (header_bytes, ticks_bytes) = data_slice.split_at_mut(DynTickArrayState::HEADER_LEN);

                // 将字节切片转换为对应的可变结构体引用
                let header: &mut DynTickArrayState = bytemuck::from_bytes_mut(header_bytes[8..].as_mut());

                let ticks: &mut [TickState] =
                    bytemuck::try_cast_slice_mut(ticks_bytes).expect("Failed to cast ticks_bytes to TickState slice");

                (header, ticks)
            });

            if data_len != header.all_data_len() {
                return Err(ErrorCode::AccountDidNotDeserialize.into());
            }

            Ok(TickArrayContainerRefMut::Dynamic((header, ticks)))
        } else if disc_bytes == TickArrayState::DISCRIMINATOR {
            let tick_array = RefMut::map(data, |data| {
                bytemuck::from_bytes_mut(&mut data.deref_mut()[8..mem::size_of::<TickArrayState>() + 8])
            });

            Ok(TickArrayContainerRefMut::Fixed(tick_array))
        } else {
            return Err(ErrorCode::AccountDiscriminatorMismatch.into());
        }
    }
}

/// static methods
impl<'info> TickArrayContainer<'info> {
    /// Load a TickArrayState of type AccountLoader from tickarray account info, if tickarray account does not exist, then create it.
    /// `access_tick_index` is the tick index that will be accessed in this tick array, in dynamic tick array, may be have to allocate more space to store TickState.
    /// `tick_array_start_index` is the start index of this tick array
    pub fn get_or_create_tick_array(
        payer: AccountInfo<'info>,
        tick_array_account_info: AccountInfo<'info>,
        system_program: AccountInfo<'info>,
        pool_state_loader: &AccountLoader<'info, PoolState>,
        tick_array_start_index: i32,
        access_tick_index: i32,
        tick_spacing: u16,
    ) -> Result<Self> {
        require!(
            TickUtils::check_is_valid_start_index(tick_array_start_index, tick_spacing),
            ClmmErrorCode::InvalidTickIndex
        );
        require!(
            access_tick_index % i32::from(tick_spacing) == 0,
            ClmmErrorCode::TickAndSpacingNotMatch
        );

        if tick_array_account_info.owner == &system_program::ID {
            let tick_array_state_loader = Self::create_dyn_tick_array_account(
                payer,
                tick_array_account_info,
                system_program,
                pool_state_loader,
                tick_array_start_index,
                access_tick_index,
                tick_spacing,
            )?;
            return Ok(TickArrayContainer::Dynamic(tick_array_state_loader));
        } else {
            // If the account is already initialized, just load it.
            // check account owner first
            if tick_array_account_info.owner != &crate::id() {
                return Err(Error::from(ErrorCode::AccountOwnedByWrongProgram)
                    .with_pubkeys((*tick_array_account_info.owner, crate::id())));
            }

            if Self::is_match_discriminator(&tick_array_account_info, TickArrayState::DISCRIMINATOR)? {
                // fixed tick array account
                let tick_array_loader = Self::check_and_load_fix_tick_array_account(
                    tick_array_account_info,
                    pool_state_loader,
                    tick_array_start_index,
                    access_tick_index,
                    tick_spacing,
                )?;

                return Ok(TickArrayContainer::Fixed(tick_array_loader));
            } else if Self::is_match_discriminator(&tick_array_account_info, DynTickArrayState::DISCRIMINATOR)? {
                // dynamic tick array account
                let dyn_tick_array_loader = Self::check_and_load_dyn_tick_array_account(
                    payer,
                    tick_array_account_info,
                    system_program,
                    pool_state_loader,
                    tick_array_start_index,
                    access_tick_index,
                    tick_spacing,
                )?;

                return Ok(TickArrayContainer::Dynamic(dyn_tick_array_loader));
            } else {
                return Err(ErrorCode::AccountDiscriminatorMismatch.into());
            };
        };
    }

    /// Try to load a TickArrayState of type AccountLoader or DynTickArrayLoader from tickarray account info
    /// after loading, will check if the access_tick_index is in this tick array
    /// `access_tick_index` is the tick index that will be accessed in this tick array
    /// `tick_spacing` is the tick spacing of the pool
    pub fn try_from(
        tick_array_account_info: &AccountInfo<'info>,
        access_tick_index: i32,
        tick_spacing: u16,
    ) -> Result<TickArrayContainer<'info>> {
        if tick_array_account_info.owner != &crate::id() {
            return Err(Error::from(ErrorCode::AccountOwnedByWrongProgram)
                .with_pubkeys((*tick_array_account_info.owner, crate::id())));
        }

        if Self::is_match_discriminator(tick_array_account_info, TickArrayState::DISCRIMINATOR)? {
            Self::validate_and_load_fixed(tick_array_account_info, access_tick_index, tick_spacing)
        } else if Self::is_match_discriminator(tick_array_account_info, DynTickArrayState::DISCRIMINATOR)? {
            Self::validate_and_load_dynamic(tick_array_account_info, access_tick_index, tick_spacing)
        } else {
            Err(ErrorCode::AccountDiscriminatorMismatch.into())
        }
    }

    #[inline(never)]
    fn validate_and_load_fixed(
        tick_array_account_info: &AccountInfo<'info>,
        access_tick_index: i32,
        tick_spacing: u16,
    ) -> Result<TickArrayContainer<'info>> {
        // fixed tick array account
        let tick_array_loader = AccountLoad::<TickArrayState>::try_from(tick_array_account_info)?;

        // check if access_tick_index is in this tick array
        {
            let tick_array = tick_array_loader.load()?;
            TickUtils::check_tick_array_start_index(tick_array.start_tick_index, access_tick_index, tick_spacing)?;

            let offset_in_array = tick_array.get_tick_offset_in_array(access_tick_index, tick_spacing)?;

            // require!(
            //     tick_array.ticks[offset_in_array].tick != 0,
            //     ClmmErrorCode::InvalidTickIndex
            // );
        }

        Ok(TickArrayContainer::Fixed(tick_array_loader))
    }

    #[inline(never)]
    fn validate_and_load_dynamic(
        tick_array_account_info: &AccountInfo<'info>,
        access_tick_index: i32,
        tick_spacing: u16,
    ) -> Result<TickArrayContainer<'info>> {
        // dynamic tick array account
        let dyn_tick_array_loader = DynTickArrayLoader::try_from(tick_array_account_info)?;

        // check if access_tick_index is in this tick array
        {
            let (dyn_tick_header, dyn_tick_states) = dyn_tick_array_loader.load()?;
            TickUtils::check_tick_array_start_index(dyn_tick_header.start_tick_index, access_tick_index, tick_spacing)?;

            let offset_in_array = dyn_tick_header.get_tick_index_in_array(access_tick_index, tick_spacing)?;

            // require!(
            //     dyn_tick_states[offset_in_array as usize].tick != 0,
            //     ClmmErrorCode::InvalidTickIndex
            // );
        }

        Ok(TickArrayContainer::Dynamic(dyn_tick_array_loader))
    }

    /// Try to load a TickArrayState of type AccountLoader or DynTickArrayLoader from tickarray account info without checking access_tick_index
    /// This function is mainly used in decrease_liquidity_v2 instruction, where access_tick_index is not known
    /// after loading, will NOT check if the access_tick_index is in
    pub fn try_from_without_check(tick_array_account_info: &AccountInfo<'info>) -> Result<TickArrayContainer<'info>> {
        if tick_array_account_info.owner != &crate::id() {
            return Err(Error::from(ErrorCode::AccountOwnedByWrongProgram)
                .with_pubkeys((*tick_array_account_info.owner, crate::id())));
        }

        if Self::is_match_discriminator(tick_array_account_info, TickArrayState::DISCRIMINATOR)? {
            // fixed tick array account
            let tick_array_loader = AccountLoad::<TickArrayState>::try_from(tick_array_account_info)?;

            Ok(TickArrayContainer::Fixed(tick_array_loader))
        } else if Self::is_match_discriminator(tick_array_account_info, DynTickArrayState::DISCRIMINATOR)? {
            // dynamic tick array account
            let dyn_tick_array_loader = DynTickArrayLoader::try_from(tick_array_account_info)?;

            Ok(TickArrayContainer::Dynamic(dyn_tick_array_loader))
        } else {
            return Err(ErrorCode::AccountDiscriminatorMismatch.into());
        }
    }

    /// Read the discriminator of an account
    pub fn is_match_discriminator(acc_info: &AccountInfo<'info>, discriminator: &[u8]) -> Result<bool> {
        let data: &[u8] = &acc_info.try_borrow_data()?;
        if data.len() < 8 {
            return Err(ErrorCode::AccountDiscriminatorNotFound.into());
        }

        if data[0..8] == discriminator[..] {
            Ok(true)
        } else {
            Ok(false)
        }
    }
}

/// private static functions
impl<'info> TickArrayContainer<'info> {
    /// Create a dynamic TickArray account, and initialize the access_tick_index in this tick array.
    fn create_dyn_tick_array_account(
        payer: AccountInfo<'info>,
        tick_array_account_info: AccountInfo<'info>,
        system_program: AccountInfo<'info>,
        pool_state_loader: &AccountLoader<'info, PoolState>,
        tick_array_start_index: i32,
        access_tick_index: i32,
        tick_spacing: u16,
    ) -> Result<DynTickArrayLoader<'info>> {
        #[cfg(all(feature = "localnet", feature = "enable-log"))]
        msg!(
            "create_dyn_tick_array_account, tick_array_start_index: {}, access_tick_index:{}, tick_spacing: {}",
            tick_array_start_index,
            access_tick_index,
            tick_spacing
        );

        // If the account is not initialized, create it. check PDA first
        let (expect_pda_address, bump) = Pubkey::find_program_address(
            &[
                TICK_ARRAY_SEED.as_bytes(),
                pool_state_loader.key().as_ref(),
                &tick_array_start_index.to_be_bytes(),
            ],
            &crate::id(),
        );
        require_keys_eq!(expect_pda_address, tick_array_account_info.key());

        // in new version of clmm, we only create dynamic tick array account
        create_or_allocate_account(
            &crate::id(),
            payer,
            system_program,
            tick_array_account_info.clone(),
            &[
                TICK_ARRAY_SEED.as_bytes(),
                pool_state_loader.key().as_ref(),
                &tick_array_start_index.to_be_bytes(),
                &[bump],
            ],
            DynTickArrayState::FIRST_CREATE_LEN,
        )?;

        let tick_array_state_loader = DynTickArrayLoader::try_from_unchecked(&tick_array_account_info)?;
        {
            let (mut dyn_tick_header, mut dyn_tick_states) = tick_array_state_loader.load_init()?;

            require_eq!(dyn_tick_states.len(), 1);

            dyn_tick_header.initialize(tick_array_start_index, tick_spacing, pool_state_loader.key())?;
            let tick_state_index = dyn_tick_header.use_one_tick(access_tick_index, tick_spacing)?;
            dyn_tick_states[tick_state_index as usize].tick = access_tick_index;
        }

        Ok(tick_array_state_loader)
    }

    /// Check and load a fixed TickArray account, and initialize the access_tick_index in this tick array if not initialized.
    fn check_and_load_fix_tick_array_account(
        tick_array_account_info: AccountInfo<'info>,
        pool_state_loader: &AccountLoader<'info, PoolState>,
        tick_array_start_index: i32,
        access_tick_index: i32,
        tick_spacing: u16,
    ) -> Result<AccountLoad<'info, TickArrayState>> {
        #[cfg(all(feature = "localnet", feature = "enable-log"))]
        msg!(
            "check_and_load_fix_tick_array_account, tick_array_start_index: {}, access_tick_index:{}, tick_spacing: {}",
            tick_array_start_index,
            access_tick_index,
            tick_spacing
        );

        let tick_array_loader = AccountLoad::<TickArrayState>::try_from(&tick_array_account_info)?;

        {
            let mut tick_array = tick_array_loader.load_mut()?;
            require_eq!(
                tick_array.start_tick_index,
                tick_array_start_index,
                ClmmErrorCode::InvalidTickArray
            );

            require_eq!(
                tick_array.pool_id,
                pool_state_loader.key(),
                ClmmErrorCode::InvalidTickArray
            );

            // initialize tick state if not initialized
            let offset_in_array =
                TickUtils::get_tick_offset_in_tick_array(tick_array.start_tick_index, access_tick_index, tick_spacing)?;

            if tick_array.ticks[offset_in_array].tick == 0 {
                tick_array.ticks[offset_in_array].tick = access_tick_index;
            }
        }

        Ok(tick_array_loader)
    }

    /// Check and load a dynamic TickArray account,
    /// if access_tick_index is not initialized, and there is no more space to initialize it, then reallocate the account to add one more TickState.
    fn check_and_load_dyn_tick_array_account(
        payer: AccountInfo<'info>,
        tick_array_account_info: AccountInfo<'info>,
        system_program: AccountInfo<'info>,
        pool_state_loader: &AccountLoader<'info, PoolState>,
        tick_array_start_index: i32,
        access_tick_index: i32,
        tick_spacing: u16,
    ) -> Result<DynTickArrayLoader<'info>> {
        #[cfg(all(feature = "localnet", feature = "enable-log"))]
        msg!(
            "check_and_load_dyn_tick_array_account, tick_array_account: {}, tick_array_start_index: {}, access_tick_index:{}, tick_spacing: {}",
            tick_array_account_info.key.to_string(),
            tick_array_start_index,
            access_tick_index,
            tick_spacing
        );

        // dynamic tick array account
        let dyn_tick_array_loader = DynTickArrayLoader::try_from(&tick_array_account_info)?;

        let mut need_add_one_more_tick_state = false;
        let tick_array_account_size;
        {
            let (dyn_tick_header, _) = dyn_tick_array_loader.load()?;
            require_eq!(
                dyn_tick_header.start_tick_index,
                tick_array_start_index,
                ClmmErrorCode::InvalidTickArray
            );
            require_eq!(
                dyn_tick_header.pool_id,
                pool_state_loader.key(),
                ClmmErrorCode::InvalidTickArray
            );

            // check if access_tick_index is already initialized
            let offset_in_array = TickUtils::get_tick_offset_in_tick_array(
                dyn_tick_header.start_tick_index,
                access_tick_index,
                tick_spacing,
            )?;
            // !offset_in_array, 实际上是原始 array 中的索引位置，还需要转换一次，才能是 dyn-tick-array 中的索引位置
            if dyn_tick_header.tick_offset_index[offset_in_array] == 0 {
                // we need to initialize this tick state, so has to add one more tick state
                need_add_one_more_tick_state = true;
            }
            tick_array_account_size = dyn_tick_header.all_data_len();
            require_eq!(tick_array_account_size, tick_array_account_info.data_len())
        }

        if need_add_one_more_tick_state {
            // reallocate the account to add one more TickState
            let new_account_space = tick_array_account_size + TickState::LEN;
            realloc_account_if_needed(&tick_array_account_info, new_account_space, &payer, &system_program)?;

            let new_dyn_tick_array_loader = DynTickArrayLoader::try_from(&tick_array_account_info)?;
            {
                let (mut dyn_tick_header, mut dyn_tick_state) = new_dyn_tick_array_loader.load_mut(true)?;

                let array_index = dyn_tick_header.use_one_tick(access_tick_index, tick_spacing)?;
                dyn_tick_state[array_index as usize].tick = access_tick_index;
                // !这里只是开辟 TickState 空间，并在header中标记该tick已被使用，具体的 TickState 初始化留到后续使用时进行
            }

            Ok(new_dyn_tick_array_loader)
        } else {
            Ok(dyn_tick_array_loader)
        }
    }
}

/// member methods for non-mutable reference
impl<'info> TickArrayContainer<'info> {
    /// Get the Pubkey of this tick array account
    pub fn key(&self) -> Result<Pubkey> {
        let (pool_id, start_tick_index) = match self {
            TickArrayContainer::Fixed(loader) => {
                let tick_array = loader.load()?;
                (tick_array.pool_id, tick_array.start_tick_index)
            }
            TickArrayContainer::Dynamic(loader) => {
                let (header, _) = loader.load()?;
                (header.pool_id, header.start_tick_index)
            }
        };

        let (pda, _bump) = Pubkey::find_program_address(
            &[
                TICK_ARRAY_SEED.as_bytes(),
                pool_id.as_ref(),
                &start_tick_index.to_be_bytes(),
            ],
            &crate::id(),
        );

        Ok(pda)
    }

    /// Get pool id of this tick array account
    pub fn get_pool_id(&self) -> Result<Pubkey> {
        match self {
            TickArrayContainer::Fixed(loader) => {
                let tick_array = loader.load()?;
                Ok(tick_array.pool_id)
            }
            TickArrayContainer::Dynamic(loader) => {
                let (header, _) = loader.load()?;
                Ok(header.pool_id)
            }
        }
    }

    /// get how many ticks are initialized in this tick array
    /// the meaning of initialized is that tick_state.liquidity_gross > 0
    pub fn get_initialized_tick_count(&self) -> Result<u8> {
        let initialized_tick_count = match self {
            TickArrayContainer::Fixed(loader) => {
                let tick_array = loader.load()?;
                tick_array.initialized_tick_count
            }
            TickArrayContainer::Dynamic(loader) => {
                let (header, _) = loader.load()?;
                header.initialized_tick_count
            }
        };

        Ok(initialized_tick_count)
    }

    /// get the start tick index of this tick array
    pub fn get_start_tick_index(&self) -> Result<i32> {
        let start_tick_index = match self {
            TickArrayContainer::Fixed(loader) => {
                let tick_array = loader.load()?;
                tick_array.start_tick_index
            }
            TickArrayContainer::Dynamic(loader) => {
                let (header, _) = loader.load()?;
                header.start_tick_index
            }
        };

        Ok(start_tick_index)
    }
}

/// member methods for non-mutable reference
impl<'info> TickArrayContainerRefMut<'_> {
    /// Get the Pubkey of this tick array account
    pub fn key(&self) -> Pubkey {
        let (pool_id, start_tick_index) = match self {
            TickArrayContainerRefMut::Fixed(tick_array) => (tick_array.pool_id, tick_array.start_tick_index),
            TickArrayContainerRefMut::Dynamic((header, _)) => (header.pool_id, header.start_tick_index),
        };

        let (pda, _bump) = Pubkey::find_program_address(
            &[
                TICK_ARRAY_SEED.as_bytes(),
                pool_id.as_ref(),
                &start_tick_index.to_be_bytes(),
            ],
            &crate::id(),
        );

        pda
    }

    /// Get pool id of this tick array account
    pub fn get_pool_id(&self) -> Pubkey {
        match self {
            TickArrayContainerRefMut::Fixed(tick_array) => tick_array.pool_id,
            TickArrayContainerRefMut::Dynamic((header, _)) => header.pool_id,
        }
    }

    /// get how many ticks are initialized in this tick array
    /// the meaning of initialized is that tick_state.liquidity_gross > 0
    pub fn get_initialized_tick_count(&self) -> u8 {
        match self {
            TickArrayContainerRefMut::Fixed(tick_array) => tick_array.initialized_tick_count,
            TickArrayContainerRefMut::Dynamic((header, _)) => header.initialized_tick_count,
        }
    }

    /// get the start tick index of this tick array
    pub fn get_start_tick_index(&self) -> i32 {
        match self {
            TickArrayContainerRefMut::Fixed(tick_array) => tick_array.start_tick_index,
            TickArrayContainerRefMut::Dynamic((header, _)) => header.start_tick_index,
        }
    }
}

/// member methods for mutable reference
impl TickArrayContainerRefMut<'_> {
    /// Get mutable reference to TickState for a given tick_index in this tick array
    pub fn get_tick_state_mut(&mut self, tick_index: i32, tick_spacing: u16) -> Result<&mut TickState> {
        match self {
            TickArrayContainerRefMut::Fixed(tick_array) => Ok(tick_array.get_tick_state_mut(tick_index, tick_spacing)?),
            TickArrayContainerRefMut::Dynamic((header, states)) => {
                let index = header.get_tick_index_in_array(tick_index, tick_spacing)? as usize;

                Ok(&mut states[index])
            }
        }
    }

    /// Update the TickState for a given tick_index in this tick array
    pub fn update_tick_state(&mut self, tick_index: i32, tick_spacing: u16, tick_state: &TickState) -> Result<()> {
        match self {
            TickArrayContainerRefMut::Fixed(tick_array) => {
                tick_array.update_tick_state(tick_index, tick_spacing, tick_state)
            }
            TickArrayContainerRefMut::Dynamic((header, states)) => {
                let index = header.get_tick_index_in_array(tick_index, tick_spacing)? as usize;
                states[index] = *tick_state;
                header.recent_epoch = get_recent_epoch()?;

                Ok(())
            }
        }
    }

    /// Update the initialized_tick_count in this tick array
    pub fn update_initialized_tick_count(&mut self, add: bool) -> Result<()> {
        match self {
            TickArrayContainerRefMut::Fixed(tick_array) => {
                if add {
                    tick_array.initialized_tick_count += 1;
                } else {
                    tick_array.initialized_tick_count -= 1;
                }
            }
            TickArrayContainerRefMut::Dynamic((header, _)) => {
                if add {
                    header.initialized_tick_count += 1;
                } else {
                    header.initialized_tick_count -= 1;
                }
            }
        }

        Ok(())
    }

    /// Get next initialized tick in tick array, `current_tick_index` can be any tick index, in other words, `current_tick_index` not exactly a point in the tickarray,
    /// and current_tick_index % tick_spacing maybe not equal zero.
    /// If price move to left tick <= current_tick_index, or to right tick > current_tick_index
    pub fn next_initialized_tick(
        &mut self,
        current_tick_index: i32,
        tick_spacing: u16,
        zero_for_one: bool,
    ) -> Result<Option<&mut TickState>> {
        match self {
            TickArrayContainerRefMut::Fixed(tick_array) => {
                tick_array.next_initialized_tick(current_tick_index, tick_spacing, zero_for_one)
            }
            TickArrayContainerRefMut::Dynamic((header, states)) => {
                let index =
                    header.next_initialized_tick_index(&states, current_tick_index, tick_spacing, zero_for_one)?;

                if let Some(i) = index {
                    Ok(Some(&mut states[i as usize]))
                } else {
                    Ok(None)
                }
            }
        }
    }

    /// Base on swap directioin, return the first initialized tick in the tick array.
    pub fn first_initialized_tick(&mut self, zero_for_one: bool) -> Result<&mut TickState> {
        match self {
            TickArrayContainerRefMut::Fixed(tick_array) => tick_array.first_initialized_tick(zero_for_one),
            TickArrayContainerRefMut::Dynamic((header, states)) => {
                let index = header.first_initialized_tick_index(&states, zero_for_one)? as usize;

                Ok(&mut states[index])
            }
        }
    }
}

#[cfg(test)]
mod tick_array_container_tests {
    use super::*;
    use crate::libraries::mock_anchor_account_info_v3;
    use anchor_lang::solana_program::pubkey::Pubkey;

    #[test]
    fn test_is_match_discriminator() {
        let key = Pubkey::new_unique();
        let owner = crate::id();

        let dyn_tick_header = DynTickArrayState::default();
        let (account_info, _lamports_box, _data_box) =
            mock_anchor_account_info_v3(&key, &owner, &dyn_tick_header, None);

        let is_dyn =
            TickArrayContainer::is_match_discriminator(&account_info, DynTickArrayState::DISCRIMINATOR).unwrap();
        assert!(is_dyn);

        let is_fixed =
            TickArrayContainer::is_match_discriminator(&account_info, TickArrayState::DISCRIMINATOR).unwrap();
        assert!(!is_fixed);
    }
}
