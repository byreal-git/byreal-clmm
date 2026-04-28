use std::cell::{Ref, RefMut};

use crate::error::ErrorCode as ClmmErrorCode;
use crate::states::{TickState, TickUtils, TICK_ARRAY_SIZE, TICK_ARRAY_SIZE_USIZE};
use crate::util::*;
use anchor_lang::error::{Error, ErrorCode};
use anchor_lang::prelude::*;
use arrayref::array_ref;

#[account(zero_copy)]
#[repr(C, packed)]
pub struct DynTickArrayState {
    pub pool_id: Pubkey,
    pub start_tick_index: i32,
    pub padding_0: [u8; 4],
    // tick_offset_index[0] is position+1 of start_tick_index;
    // tick_offset_index[n] is position+1 of start_tick_index + n * tick_spacing;
    // position: means the index in TickState array, which follows this header
    // ...
    // 0 means this tick is not allocated
    // !下标(index)就是 tick-index, 值是 TickState 数组中的位置+1, 值为0表示该tick未分配
    /// TickStateArray[tick_offset_index[0]-1] is TickState of start_tick_index
    /// TickStateArray[tick_offset_index[n]-1] is TickState of start_tick_index + n * tick_spacing
    pub tick_offset_index: [u8; TICK_ARRAY_SIZE_USIZE],
    /// how many ticks are allocated in this tick array
    pub alloc_tick_count: u8,
    /// how many ticks are initialized in this tick array
    pub initialized_tick_count: u8,
    pub padding_1: [u8; 2],
    // account update recent epoch
    pub recent_epoch: u64,
    // Unused bytes for future upgrades.
    pub padding_2: [u8; 96],
}
// TickState array, max size is TICK_ARRAY_SIZE_USIZE

impl Default for DynTickArrayState {
    fn default() -> Self {
        Self {
            pool_id: Pubkey::default(),
            start_tick_index: 0,
            padding_0: [0; 4],
            tick_offset_index: [0; TICK_ARRAY_SIZE_USIZE],
            alloc_tick_count: 0,
            initialized_tick_count: 0,
            padding_1: [0; 2],
            recent_epoch: 0,
            padding_2: [0; 96],
        }
    }
}

impl DynTickArrayState {
    pub const HEADER_LEN: usize = 8 + std::mem::size_of::<DynTickArrayState>();

    // when first create, we only allocate space for header + one TickState
    pub const FIRST_CREATE_LEN: usize = Self::HEADER_LEN + TickState::LEN;

    pub fn all_data_len(&self) -> usize {
        Self::HEADER_LEN + self.alloc_tick_count as usize * TickState::LEN
    }

    pub fn initialize(&mut self, start_index: i32, tick_spacing: u16, pool_key: Pubkey) -> Result<()> {
        TickUtils::check_is_valid_start_index(start_index, tick_spacing);
        self.start_tick_index = start_index;
        self.pool_id = pool_key;
        self.recent_epoch = get_recent_epoch()?;

        Ok(())
    }

    /// Mark a TickState as used in this tick array.
    /// return the index of this tick in the DynTickStateArray
    pub fn use_one_tick(&mut self, tick_index: i32, tick_spacing: u16) -> Result<u8> {
        require_eq!(
            TickUtils::get_array_start_index(tick_index, tick_spacing),
            self.start_tick_index,
            ClmmErrorCode::InvalidTickIndex
        );

        let offset = TickUtils::get_tick_offset_in_tick_array(self.start_tick_index, tick_index, tick_spacing)?;

        require!(self.tick_offset_index[offset] == 0, ClmmErrorCode::InvalidTickIndex);

        self.alloc_tick_count += 1;
        self.tick_offset_index[offset] = self.alloc_tick_count;

        let tick_state_index = self.alloc_tick_count - 1;

        Ok(tick_state_index)
    }

    /// Get the index of a tick in the TickState array.
    /// The TickState array is placed after the header in the account data.
    /// function like tick_array.get_tick_offset_in_array(tick_index, tick_spacing)
    pub fn get_tick_index_in_array(&self, tick_index: i32, tick_spacing: u16) -> Result<u8> {
        require_eq!(
            TickUtils::get_array_start_index(tick_index, tick_spacing),
            self.start_tick_index,
            ClmmErrorCode::InvalidTickIndex
        );

        let offset = TickUtils::get_tick_offset_in_tick_array(self.start_tick_index, tick_index, tick_spacing)?;

        let tick_state_index = self.tick_offset_index[offset];
        require!(tick_state_index > 0, ClmmErrorCode::InvalidTickIndex);

        Ok(tick_state_index - 1)
    }

    pub fn next_initialized_tick_index(
        &self,
        tick_state_slice: &[TickState],
        current_tick_index: i32,
        tick_spacing: u16,
        zero_for_one: bool,
    ) -> Result<Option<u8>> {
        let current_tick_array_start_index = TickUtils::get_array_start_index(current_tick_index, tick_spacing);
        if current_tick_array_start_index != self.start_tick_index {
            return Ok(None);
        }
        let mut offset_in_array = (current_tick_index - self.start_tick_index) / i32::from(tick_spacing);

        if zero_for_one {
            while offset_in_array >= 0 {
                if self.tick_offset_index[offset_in_array as usize] > 0
                    && tick_state_slice[self.tick_offset_index[offset_in_array as usize] as usize - 1].is_initialized()
                {
                    return Ok(Some(self.tick_offset_index[offset_in_array as usize] - 1));
                }
                offset_in_array = offset_in_array - 1;
            }
        } else {
            offset_in_array = offset_in_array + 1;
            while offset_in_array < TICK_ARRAY_SIZE {
                if self.tick_offset_index[offset_in_array as usize] > 0
                    && tick_state_slice[self.tick_offset_index[offset_in_array as usize] as usize - 1].is_initialized()
                {
                    return Ok(Some(self.tick_offset_index[offset_in_array as usize] - 1));
                }
                offset_in_array = offset_in_array + 1;
            }
        }
        Ok(None)
    }

    /// Base on swap directioin, return the first initialized tick(tick-index in dyn-tick-array) in the tick array.
    pub fn first_initialized_tick_index(&self, tick_state_slice: &[TickState], zero_for_one: bool) -> Result<u8> {
        if zero_for_one {
            let mut i = TICK_ARRAY_SIZE - 1;
            while i >= 0 {
                if self.tick_offset_index[i as usize] > 0
                    && tick_state_slice[self.tick_offset_index[i as usize] as usize - 1].is_initialized()
                {
                    return Ok(self.tick_offset_index[i as usize] - 1);
                }
                i = i - 1;
            }
        } else {
            let mut i = 0;
            while i < TICK_ARRAY_SIZE_USIZE {
                if self.tick_offset_index[i] > 0
                    && tick_state_slice[self.tick_offset_index[i] as usize - 1].is_initialized()
                {
                    return Ok(self.tick_offset_index[i] - 1);
                }
                i = i + 1;
            }
        }
        err!(ClmmErrorCode::InvalidTickArray)
    }

    /// Base on swap directioin, return the next tick array start index.
    pub fn next_tick_arrary_start_index(&self, tick_spacing: u16, zero_for_one: bool) -> i32 {
        let ticks_in_array = TICK_ARRAY_SIZE * i32::from(tick_spacing);
        if zero_for_one {
            self.start_tick_index - ticks_in_array
        } else {
            self.start_tick_index + ticks_in_array
        }
    }
}

/// Loader for dynamic TickArray accounts
#[derive(Clone)]
pub struct DynTickArrayLoader<'info> {
    acc_info: AccountInfo<'info>,
}

/// static methods
impl<'info> DynTickArrayLoader<'info> {
    pub fn new(acc_info: AccountInfo<'info>) -> Self {
        Self { acc_info }
    }

    /// Constructs a new `Loader` from a previously initialized account.
    #[inline(never)]
    pub fn try_from(acc_info: &AccountInfo<'info>) -> Result<Self> {
        if acc_info.owner != &crate::id() {
            msg!(
                "account owner error, account={}, {}:{}",
                acc_info.key(),
                file!(),
                line!()
            );
            return Err(Error::from(ErrorCode::AccountOwnedByWrongProgram).with_pubkeys((*acc_info.owner, crate::id())));
        }
        let data: &[u8] = &acc_info.try_borrow_data()?;
        if data.len() < DynTickArrayState::DISCRIMINATOR.len() {
            return Err(ErrorCode::AccountDiscriminatorNotFound.into());
        }
        // Discriminator must match.
        let disc_bytes = array_ref![data, 0, 8];
        if disc_bytes != &DynTickArrayState::DISCRIMINATOR {
            msg!(
                "account discriminator mismatch, account={}, {}:{}",
                acc_info.key(),
                file!(),
                line!()
            );
            return Err(ErrorCode::AccountDiscriminatorMismatch.into());
        }

        Ok(Self::new(acc_info.clone()))
    }

    /// Constructs a new `Loader` from an uninitialized account.
    #[inline(never)]
    pub fn try_from_unchecked(acc_info: &AccountInfo<'info>) -> Result<Self> {
        if acc_info.owner != &crate::id() {
            msg!(
                "account owner error, account={}, {}:{}",
                acc_info.key(),
                file!(),
                line!()
            );
            return Err(Error::from(ErrorCode::AccountOwnedByWrongProgram).with_pubkeys((*acc_info.owner, crate::id())));
        }
        Ok(Self::new(acc_info.clone()))
    }
}

/// member methods
impl<'info> DynTickArrayLoader<'info> {
    /// Returns a `RefMut` to the account data structure for reading or writing.
    /// Should only be called once, when the account is being initialized.
    pub fn load_init<'a>(&'a self) -> Result<(RefMut<'a, DynTickArrayState>, RefMut<'a, [TickState]>)> {
        // AccountInfo api allows you to borrow mut even if the account isn't
        // writable, so add this check for a better dev experience.
        if !self.acc_info.is_writable {
            msg!(
                "account not mutable, account={}, {}:{}",
                self.acc_info.key(),
                file!(),
                line!()
            );
            return Err(ErrorCode::AccountNotMutable.into());
        }

        let mut data = self.acc_info.try_borrow_mut_data()?;

        // The discriminator should be zero, since we're initializing.
        let mut disc_bytes = [0u8; 8];
        disc_bytes.copy_from_slice(&data[..8]);
        let discriminator = u64::from_le_bytes(disc_bytes);
        if discriminator != 0 {
            msg!(
                "account discriminator already set, account={}, {}:{}",
                self.acc_info.key(),
                file!(),
                line!()
            );
            return Err(ErrorCode::AccountDiscriminatorAlreadySet.into());
        }

        // write discriminator
        data[..8].copy_from_slice(&DynTickArrayState::DISCRIMINATOR);

        // split the data into header and ticks part
        if data.len() < DynTickArrayState::HEADER_LEN {
            return Err(ErrorCode::AccountDidNotDeserialize.into());
        }

        let (header, ticks) = RefMut::map_split(data, |data_slice| {
            let (header_bytes, ticks_bytes) = data_slice.split_at_mut(DynTickArrayState::HEADER_LEN);

            // 将字节切片转换为对应的可变结构体引用
            let header: &mut DynTickArrayState = bytemuck::from_bytes_mut(header_bytes[8..].as_mut());

            let ticks: &mut [TickState] =
                bytemuck::try_cast_slice_mut(ticks_bytes).expect("Failed to cast ticks_bytes to TickState slice");

            (header, ticks)
        });

        Ok((header, ticks))
    }

    /// Returns a `RefMut` to the account data structure for reading or writing.
    /// Should only be called once, when the account is being initialized.
    /// `is_after_resize`: indicate whether the account has been resized before calling this method.
    pub fn load_mut<'a>(
        &'a self,
        is_after_resize: bool,
    ) -> Result<(RefMut<'a, DynTickArrayState>, RefMut<'a, [TickState]>)> {
        // AccountInfo api allows you to borrow mut even if the account isn't
        // writable, so add this check for a better dev experience.
        if !self.acc_info.is_writable {
            msg!(
                "account not mutable, account={}, {}:{}",
                self.acc_info.key(),
                file!(),
                line!()
            );
            return Err(ErrorCode::AccountNotMutable.into());
        }

        let data = self.acc_info.try_borrow_mut_data()?;
        let data_len = data.len();

        // check discriminator
        {
            if data_len < DynTickArrayState::DISCRIMINATOR.len() {
                return Err(ErrorCode::AccountDiscriminatorNotFound.into());
            }
            let disc_bytes = array_ref![data, 0, 8];
            if disc_bytes != &DynTickArrayState::DISCRIMINATOR {
                msg!(
                    "account discriminator mismatch, account={}, {}:{}",
                    self.acc_info.key(),
                    file!(),
                    line!()
                );
                return Err(ErrorCode::AccountDiscriminatorMismatch.into());
            }
        }

        let (header, ticks) = RefMut::map_split(data, |data_slice| {
            let (header_bytes, ticks_bytes) = data_slice.split_at_mut(DynTickArrayState::HEADER_LEN);

            // 将字节切片转换为对应的可变结构体引用
            let header: &mut DynTickArrayState = bytemuck::from_bytes_mut(header_bytes[8..].as_mut());

            let ticks: &mut [TickState] =
                bytemuck::try_cast_slice_mut(ticks_bytes).expect("Failed to cast ticks_bytes to TickState slice");

            (header, ticks)
        });

        // ! 对账户进行 resize 后, 再次 deserialize 时, 数据长度和 header 中记录的长度会不一致
        if !is_after_resize {
            if data_len != header.all_data_len() {
                return Err(ErrorCode::AccountDidNotDeserialize.into());
            }
        }

        Ok((header, ticks))
    }

    /// Returns a Ref to the account data structure for reading.
    pub fn load<'a>(&'a self) -> Result<(Ref<'a, DynTickArrayState>, Ref<'a, [TickState]>)> {
        let data = self.acc_info.try_borrow_data()?;
        let data_len = data.len();

        {
            if data_len < DynTickArrayState::DISCRIMINATOR.len() {
                return Err(ErrorCode::AccountDiscriminatorNotFound.into());
            }

            let disc_bytes = array_ref![data, 0, 8];
            if disc_bytes != &DynTickArrayState::DISCRIMINATOR {
                msg!(
                    "account discriminator mismatch, account={}, {}:{}",
                    self.acc_info.key(),
                    file!(),
                    line!()
                );
                return Err(ErrorCode::AccountDiscriminatorMismatch.into());
            }
        }

        let (header, ticks) = Ref::map_split(data, |data_slice| {
            let (header_bytes, ticks_bytes) = data_slice.split_at(DynTickArrayState::HEADER_LEN);

            // 将字节切片转换为对应的可变结构体引用
            let header: &DynTickArrayState = bytemuck::from_bytes(header_bytes[8..].as_ref());

            let ticks: &[TickState] =
                bytemuck::try_cast_slice(ticks_bytes).expect("Failed to cast ticks_bytes to TickState slice");

            (header, ticks)
        });

        if data_len != header.all_data_len() {
            return Err(ErrorCode::AccountDidNotDeserialize.into());
        }

        Ok((header, ticks))
    }
}

#[cfg(test)]
pub mod dyn_tick_array_test {
    use rand::{seq::SliceRandom, thread_rng};

    use super::*;
    use std::cell::RefCell;

    /// Dynamic Tick Array Build Type
    pub enum DynamicTickArrayBuildType {
        /// tick-state 的第一个元素是 start_tick_index,
        FromStartIndex,
        /// tick-state 的第一个元素是 end_tick_index
        FromEndIndex,
        /// tick-state 的元素是随机的 tick-index
        RandomIndex,
    }

    pub struct DynTickArrayInfo {
        pub start_tick_index: i32,
        pub build_type: DynamicTickArrayBuildType,
        pub ticks: Vec<TickState>,
    }

    pub fn build_dyn_tick_array(
        start_index: i32,
        tick_spacing: u16,
        build_type: DynamicTickArrayBuildType,
        initialized_tick_offsets: Vec<usize>,
    ) -> (RefCell<DynTickArrayState>, RefCell<Vec<TickState>>) {
        let mut dyn_tick_header = DynTickArrayState::default();
        dyn_tick_header
            .initialize(start_index, tick_spacing, Pubkey::default())
            .unwrap();

        let mut dyn_tick_states = vec![];

        let tick_offsets = match build_type {
            DynamicTickArrayBuildType::FromEndIndex => {
                // 降序排列
                let mut a = initialized_tick_offsets.clone();
                a.sort_by(|a, b| b.cmp(a));
                a
            }
            DynamicTickArrayBuildType::FromStartIndex => {
                // 升序排列
                let mut a = initialized_tick_offsets.clone();
                a.sort();
                a
            }
            DynamicTickArrayBuildType::RandomIndex => {
                // 随机顺序
                let mut a = initialized_tick_offsets.clone();
                let mut rng = thread_rng();
                a.shuffle(&mut rng);
                a
            }
        };

        for offset in tick_offsets {
            let mut new_tick = TickState::default();
            // Indicates tick is initialized
            new_tick.liquidity_gross = 1;
            new_tick.tick = start_index + (offset * tick_spacing as usize) as i32;

            // 使用了 1 个 tick
            dyn_tick_header.use_one_tick(new_tick.tick, tick_spacing).unwrap();

            dyn_tick_states.push(new_tick);
        }

        (RefCell::new(dyn_tick_header), RefCell::new(dyn_tick_states))
    }

    pub fn build_dyn_tick_array_with_tick_states(
        pool_id: Pubkey,
        start_index: i32,
        tick_spacing: u16,
        build_type: DynamicTickArrayBuildType,
        tick_states: Vec<TickState>,
    ) -> (RefCell<DynTickArrayState>, RefCell<Vec<TickState>>) {
        let mut dyn_tick_header = DynTickArrayState::default();
        dyn_tick_header.initialize(start_index, tick_spacing, pool_id).unwrap();

        let mut dyn_tick_states = vec![];

        let sorted_tick_states = match build_type {
            DynamicTickArrayBuildType::FromEndIndex => {
                // 降序排列
                let mut a = tick_states.clone();
                a.sort_by(|a, b| {
                    let a_tick = a.tick;
                    let b_tick = b.tick;
                    b_tick.cmp(&a_tick)
                });
                a
            }
            DynamicTickArrayBuildType::FromStartIndex => {
                // 升序排列
                let mut a = tick_states.clone();
                a.sort_by(|a, b| {
                    let a_tick = a.tick;
                    let b_tick = b.tick;

                    a_tick.cmp(&b_tick)
                });
                a
            }
            DynamicTickArrayBuildType::RandomIndex => {
                // 随机顺序
                let mut a = tick_states.clone();
                let mut rng = thread_rng();
                a.shuffle(&mut rng);
                a
            }
        };

        for tick_state in sorted_tick_states {
            dyn_tick_header.use_one_tick(tick_state.tick, tick_spacing).unwrap();
            dyn_tick_states.push(tick_state);
        }

        (RefCell::new(dyn_tick_header), RefCell::new(dyn_tick_states))
    }

    pub fn build_tick(tick: i32, liquidity_gross: u128, liquidity_net: i128) -> RefCell<TickState> {
        let mut new_tick = TickState::default();
        new_tick.tick = tick;
        new_tick.liquidity_gross = liquidity_gross;
        new_tick.liquidity_net = liquidity_net;
        RefCell::new(new_tick)
    }

    fn build_tick_with_fee_reward_growth(
        tick: i32,
        fee_growth_outside_0_x64: u128,
        fee_growth_outside_1_x64: u128,
        reward_growths_outside_x64: u128,
    ) -> RefCell<TickState> {
        let mut new_tick = TickState::default();
        new_tick.tick = tick;
        new_tick.fee_growth_outside_0_x64 = fee_growth_outside_0_x64;
        new_tick.fee_growth_outside_1_x64 = fee_growth_outside_1_x64;
        new_tick.reward_growths_outside_x64 = [reward_growths_outside_x64, 0, 0];
        RefCell::new(new_tick)
    }

    mod dyn_tick_array_test {
        use super::*;
        use crate::libraries::tick_math;
        use std::convert::identity;

        #[test]
        fn get_dyn_array_start_index_test() {
            assert_eq!(TickUtils::get_array_start_index(120, 3), 0);
            assert_eq!(TickUtils::get_array_start_index(1002, 30), 0);
            assert_eq!(TickUtils::get_array_start_index(-120, 3), -180);
            assert_eq!(TickUtils::get_array_start_index(-1002, 30), -1800);
            assert_eq!(TickUtils::get_array_start_index(-20, 10), -600);
            assert_eq!(TickUtils::get_array_start_index(20, 10), 0);
            assert_eq!(TickUtils::get_array_start_index(-1002, 10), -1200);
            assert_eq!(TickUtils::get_array_start_index(-600, 10), -600);
            assert_eq!(TickUtils::get_array_start_index(-30720, 1), -30720);
            assert_eq!(TickUtils::get_array_start_index(30720, 1), 30720);
            assert_eq!(TickUtils::get_array_start_index(tick_math::MIN_TICK, 1), -443640);
            assert_eq!(TickUtils::get_array_start_index(tick_math::MAX_TICK, 1), 443580);
            assert_eq!(TickUtils::get_array_start_index(tick_math::MAX_TICK, 60), 442800);
            assert_eq!(TickUtils::get_array_start_index(tick_math::MIN_TICK, 60), -446400);
        }

        #[test]
        fn next_tick_arrary_start_index_test() {
            let tick_spacing = 15;
            let (dyn_tick_header, _) =
                build_dyn_tick_array(-1800, tick_spacing, DynamicTickArrayBuildType::FromEndIndex, vec![]);
            // zero_for_one, next tickarray start_index < current
            assert_eq!(
                -2700,
                dyn_tick_header
                    .borrow()
                    .next_tick_arrary_start_index(tick_spacing, true)
            );
            // one_for_zero, next tickarray start_index > current
            assert_eq!(
                -900,
                dyn_tick_header
                    .borrow()
                    .next_tick_arrary_start_index(tick_spacing, false)
            );
        }

        #[test]
        fn get_tick_index_in_array_test() {
            let tick_spacing = 4;
            // tick range [960, 1196]
            let (dyn_tick_header, _) =
                build_dyn_tick_array(960, tick_spacing, DynamicTickArrayBuildType::FromStartIndex, vec![]);

            // not in tickarray
            assert_eq!(
                dyn_tick_header
                    .borrow()
                    .get_tick_index_in_array(808, tick_spacing)
                    .unwrap_err(),
                error!(ClmmErrorCode::InvalidTickIndex)
            );

            // first index is tickarray start tick
            let array_index = dyn_tick_header.borrow_mut().use_one_tick(960, tick_spacing).unwrap();
            assert_eq!(dyn_tick_header.borrow().tick_offset_index[0], array_index + 1);
            assert_eq!(
                dyn_tick_header
                    .borrow()
                    .get_tick_index_in_array(960, tick_spacing)
                    .unwrap(),
                array_index
            );

            // tick_index % tick_spacing != 0
            let array_index = dyn_tick_header.borrow_mut().use_one_tick(1105, tick_spacing).unwrap();
            assert_eq!(dyn_tick_header.borrow().tick_offset_index[36], array_index + 1);
            assert_eq!(
                dyn_tick_header
                    .borrow()
                    .get_tick_index_in_array(1105, tick_spacing)
                    .unwrap(),
                array_index
            );

            // (1108-960) / tick_spacing
            let array_index = dyn_tick_header.borrow_mut().use_one_tick(1108, tick_spacing).unwrap();
            assert_eq!(dyn_tick_header.borrow().tick_offset_index[37], array_index + 1);
            assert_eq!(
                dyn_tick_header
                    .borrow()
                    .get_tick_index_in_array(1108, tick_spacing)
                    .unwrap(),
                array_index
            );

            // the end index of tickarray
            let array_index = dyn_tick_header.borrow_mut().use_one_tick(1196, tick_spacing).unwrap();
            assert_eq!(dyn_tick_header.borrow().tick_offset_index[59], array_index + 1);

            assert_eq!(
                dyn_tick_header
                    .borrow()
                    .get_tick_index_in_array(1196, tick_spacing)
                    .unwrap(),
                array_index
            );
        }

        #[test]
        fn first_initialized_tick_test() {
            let tick_spacing = 15;

            // initialized ticks[-300,-15]
            let (dyn_tick_header, dyn_tick_state) = build_dyn_tick_array(
                -900,
                tick_spacing,
                DynamicTickArrayBuildType::FromStartIndex,
                vec![40, 59],
            );

            let tick_array = dyn_tick_state.borrow_mut();

            // one_for_zero, the price increase, tick from small to large
            let arry_index = dyn_tick_header
                .borrow()
                .first_initialized_tick_index(&tick_array, false)
                .unwrap() as usize;
            let tick = tick_array[arry_index].tick;
            assert_eq!(-300, tick);

            // zero_for_one, the price decrease, tick from large to small
            let arry_index = dyn_tick_header
                .borrow()
                .first_initialized_tick_index(&tick_array, true)
                .unwrap() as usize;
            let tick = tick_array[arry_index].tick;
            assert_eq!(-15, tick);
        }

        #[test]
        fn next_initialized_tick_when_tick_is_positive() {
            // init tick_index [0,30,105]
            let (dyn_tick_header, dyn_tick_state) =
                build_dyn_tick_array(0, 15, DynamicTickArrayBuildType::FromStartIndex, vec![0, 2, 7]);
            let tick_array = dyn_tick_state.borrow_mut();

            // test zero_for_one
            let mut array_index = dyn_tick_header
                .borrow()
                .next_initialized_tick_index(&tick_array, 0, 15, true)
                .unwrap()
                .unwrap() as usize;
            let mut next_tick_state = tick_array[array_index];
            assert_eq!(identity(next_tick_state.tick), 0);

            array_index = dyn_tick_header
                .borrow()
                .next_initialized_tick_index(&tick_array, 1, 15, true)
                .unwrap()
                .unwrap() as usize;
            next_tick_state = tick_array[array_index];
            assert_eq!(identity(next_tick_state.tick), 0);

            array_index = dyn_tick_header
                .borrow()
                .next_initialized_tick_index(&tick_array, 29, 15, true)
                .unwrap()
                .unwrap() as usize;
            next_tick_state = tick_array[array_index];
            assert_eq!(identity(next_tick_state.tick), 0);

            array_index = dyn_tick_header
                .borrow()
                .next_initialized_tick_index(&tick_array, 30, 15, true)
                .unwrap()
                .unwrap() as usize;
            next_tick_state = tick_array[array_index];
            assert_eq!(identity(next_tick_state.tick), 30);

            array_index = dyn_tick_header
                .borrow()
                .next_initialized_tick_index(&tick_array, 31, 15, true)
                .unwrap()
                .unwrap() as usize;
            next_tick_state = tick_array[array_index];
            assert_eq!(identity(next_tick_state.tick), 30);

            // test one for zero
            let mut array_index = dyn_tick_header
                .borrow()
                .next_initialized_tick_index(&tick_array, 0, 15, false)
                .unwrap()
                .unwrap() as usize;
            let mut next_tick_state = tick_array[array_index];
            assert_eq!(identity(next_tick_state.tick), 30);

            array_index = dyn_tick_header
                .borrow()
                .next_initialized_tick_index(&tick_array, 29, 15, false)
                .unwrap()
                .unwrap() as usize;
            next_tick_state = tick_array[array_index];
            assert_eq!(identity(next_tick_state.tick), 30);

            array_index = dyn_tick_header
                .borrow()
                .next_initialized_tick_index(&tick_array, 30, 15, false)
                .unwrap()
                .unwrap() as usize;
            next_tick_state = tick_array[array_index];
            assert_eq!(identity(next_tick_state.tick), 105);

            array_index = dyn_tick_header
                .borrow()
                .next_initialized_tick_index(&tick_array, 31, 15, false)
                .unwrap()
                .unwrap() as usize;
            next_tick_state = tick_array[array_index];
            assert_eq!(identity(next_tick_state.tick), 105);

            let array_index = dyn_tick_header
                .borrow()
                .next_initialized_tick_index(&tick_array, 105, 15, false)
                .unwrap();
            assert!(array_index.is_none());

            // tick not in tickarray
            let array_index = dyn_tick_header
                .borrow()
                .next_initialized_tick_index(&tick_array, 900, 15, false)
                .unwrap();
            assert!(array_index.is_none());
        }

        #[test]
        fn next_initialized_tick_when_tick_is_negative() {
            // init tick_index [-900,-870,-795]
            let (dyn_tick_header, dyn_tick_state) =
                build_dyn_tick_array(-900, 15, DynamicTickArrayBuildType::FromEndIndex, vec![0, 2, 7]);
            let tick_array = dyn_tick_state.borrow_mut();

            // test zero for one
            let mut array_index = dyn_tick_header
                .borrow()
                .next_initialized_tick_index(&tick_array, -900, 15, true)
                .unwrap()
                .unwrap() as usize;
            let mut next_tick_state = tick_array[array_index];
            assert_eq!(identity(next_tick_state.tick), -900);

            array_index = dyn_tick_header
                .borrow()
                .next_initialized_tick_index(&tick_array, -899, 15, true)
                .unwrap()
                .unwrap() as usize;
            next_tick_state = tick_array[array_index];
            assert_eq!(identity(next_tick_state.tick), -900);

            array_index = dyn_tick_header
                .borrow()
                .next_initialized_tick_index(&tick_array, -871, 15, true)
                .unwrap()
                .unwrap() as usize;
            next_tick_state = tick_array[array_index];
            assert_eq!(identity(next_tick_state.tick), -900);

            array_index = dyn_tick_header
                .borrow()
                .next_initialized_tick_index(&tick_array, -870, 15, true)
                .unwrap()
                .unwrap() as usize;
            next_tick_state = tick_array[array_index];
            assert_eq!(identity(next_tick_state.tick), -870);

            array_index = dyn_tick_header
                .borrow()
                .next_initialized_tick_index(&tick_array, -869, 15, true)
                .unwrap()
                .unwrap() as usize;
            next_tick_state = tick_array[array_index];
            assert_eq!(identity(next_tick_state.tick), -870);

            // test one for zero
            let mut array_index = dyn_tick_header
                .borrow()
                .next_initialized_tick_index(&tick_array, -900, 15, false)
                .unwrap()
                .unwrap() as usize;
            let mut next_tick_state = tick_array[array_index];
            assert_eq!(identity(next_tick_state.tick), -870);

            array_index = dyn_tick_header
                .borrow()
                .next_initialized_tick_index(&tick_array, -871, 15, false)
                .unwrap()
                .unwrap() as usize;
            next_tick_state = tick_array[array_index];
            assert_eq!(identity(next_tick_state.tick), -870);

            array_index = dyn_tick_header
                .borrow()
                .next_initialized_tick_index(&tick_array, -870, 15, false)
                .unwrap()
                .unwrap() as usize;
            next_tick_state = tick_array[array_index];
            assert_eq!(identity(next_tick_state.tick), -795);

            array_index = dyn_tick_header
                .borrow()
                .next_initialized_tick_index(&tick_array, -869, 15, false)
                .unwrap()
                .unwrap() as usize;
            next_tick_state = tick_array[array_index];
            assert_eq!(identity(next_tick_state.tick), -795);

            let array_index = dyn_tick_header
                .borrow()
                .next_initialized_tick_index(&tick_array, -795, 15, false)
                .unwrap();
            assert!(array_index.is_none());

            // tick not in tickarray
            let array_index = dyn_tick_header
                .borrow()
                .next_initialized_tick_index(&tick_array, -10, 15, false)
                .unwrap();
            assert!(array_index.is_none());
        }
    }

    mod get_fee_growth_inside_test {
        use super::*;
        use crate::states::*;

        fn fee_growth_inside_delta_when_price_move(
            init_fee_growth_global_0_x64: u128,
            init_fee_growth_global_1_x64: u128,
            fee_growth_global_delta: u128,
            mut tick_current: i32,
            target_tick_current: i32,
            tick_lower: &mut TickState,
            tick_upper: &mut TickState,
            cross_tick_lower: bool,
        ) -> (u128, u128) {
            let mut fee_growth_global_0_x64 = init_fee_growth_global_0_x64;
            let mut fee_growth_global_1_x64 = init_fee_growth_global_1_x64;
            let (fee_growth_inside_0_before, fee_growth_inside_1_before) = TickUtils::get_fee_growth_inside(
                tick_lower,
                tick_upper,
                tick_current,
                fee_growth_global_0_x64,
                fee_growth_global_1_x64,
            );

            if fee_growth_global_0_x64 != 0 {
                fee_growth_global_0_x64 = fee_growth_global_0_x64 + fee_growth_global_delta;
            }
            if fee_growth_global_1_x64 != 0 {
                fee_growth_global_1_x64 = fee_growth_global_1_x64 + fee_growth_global_delta;
            }
            if cross_tick_lower {
                tick_lower.cross(
                    fee_growth_global_0_x64,
                    fee_growth_global_1_x64,
                    &[RewardInfo::default(); 3],
                );
            } else {
                tick_upper.cross(
                    fee_growth_global_0_x64,
                    fee_growth_global_1_x64,
                    &[RewardInfo::default(); 3],
                );
            }

            tick_current = target_tick_current;
            let (fee_growth_inside_0_after, fee_growth_inside_1_after) = TickUtils::get_fee_growth_inside(
                tick_lower,
                tick_upper,
                tick_current,
                fee_growth_global_0_x64,
                fee_growth_global_1_x64,
            );

            println!(
                "inside_delta_0:{},fee_growth_inside_0_after:{},fee_growth_inside_0_before:{}",
                fee_growth_inside_0_after.wrapping_sub(fee_growth_inside_0_before),
                fee_growth_inside_0_after,
                fee_growth_inside_0_before
            );
            println!(
                "inside_delta_1:{},fee_growth_inside_1_after:{},fee_growth_inside_1_before:{}",
                fee_growth_inside_1_after.wrapping_sub(fee_growth_inside_1_before),
                fee_growth_inside_1_after,
                fee_growth_inside_1_before
            );
            (
                fee_growth_inside_0_after.wrapping_sub(fee_growth_inside_0_before),
                fee_growth_inside_1_after.wrapping_sub(fee_growth_inside_1_before),
            )
        }

        #[test]
        fn price_in_tick_range_move_to_right_test() {
            // one_for_zero, price move to right and token_1 fee growth

            // tick_lower and tick_upper all new create, and tick_lower initialize with fee_growth_global_1_x64(1000)
            let (fee_growth_inside_delta_0, fee_growth_inside_delta_1) = fee_growth_inside_delta_when_price_move(
                0,
                1000,
                500,
                0,
                11,
                build_tick_with_fee_reward_growth(-10, 0, 1000, 0).get_mut(),
                build_tick_with_fee_reward_growth(10, 0, 0, 0).get_mut(),
                false,
            );
            assert_eq!(fee_growth_inside_delta_0, 0);
            assert_eq!(fee_growth_inside_delta_1, 500);

            // tick_lower is initialized with fee_growth_outside_1_x64(100) and tick_upper is new create.
            let (fee_growth_inside_delta_0, fee_growth_inside_delta_1) = fee_growth_inside_delta_when_price_move(
                0,
                1000,
                500,
                0,
                11,
                build_tick_with_fee_reward_growth(-10, 0, 100, 0).get_mut(),
                build_tick_with_fee_reward_growth(10, 0, 0, 0).get_mut(),
                false,
            );
            assert_eq!(fee_growth_inside_delta_0, 0);
            assert_eq!(fee_growth_inside_delta_1, 500);

            // tick_lower is new create with fee_growth_global_1_x64(1000)  and tick_upper is initialized with fee_growth_outside_1_x64(100)
            let (fee_growth_inside_delta_0, fee_growth_inside_delta_1) = fee_growth_inside_delta_when_price_move(
                0,
                1000,
                500,
                0,
                11,
                build_tick_with_fee_reward_growth(-10, 0, 1000, 0).get_mut(),
                build_tick_with_fee_reward_growth(10, 0, 100, 0).get_mut(),
                false,
            );
            assert_eq!(fee_growth_inside_delta_0, 0);
            assert_eq!(fee_growth_inside_delta_1, 500);

            // tick_lower is initialized with fee_growth_outside_1_x64(50)  and tick_upper is initialized with fee_growth_outside_1_x64(100)
            let (fee_growth_inside_delta_0, fee_growth_inside_delta_1) = fee_growth_inside_delta_when_price_move(
                0,
                1000,
                500,
                0,
                11,
                build_tick_with_fee_reward_growth(-10, 0, 50, 0).get_mut(),
                build_tick_with_fee_reward_growth(10, 0, 100, 0).get_mut(),
                false,
            );
            assert_eq!(fee_growth_inside_delta_0, 0);
            assert_eq!(fee_growth_inside_delta_1, 500);
        }

        #[test]
        fn price_in_tick_range_move_to_left_test() {
            // zero_for_one, price move to left and token_0 fee growth

            // tick_lower and tick_upper all new create, and tick_lower initialize with fee_growth_global_0_x64(1000)
            let (fee_growth_inside_delta_0, fee_growth_inside_delta_1) = fee_growth_inside_delta_when_price_move(
                1000,
                0,
                500,
                0,
                -11,
                build_tick_with_fee_reward_growth(-10, 1000, 0, 0).get_mut(),
                build_tick_with_fee_reward_growth(10, 0, 0, 0).get_mut(),
                true,
            );
            assert_eq!(fee_growth_inside_delta_0, 500);
            assert_eq!(fee_growth_inside_delta_1, 0);

            // tick_lower is initialized with fee_growth_outside_0_x64(100) and tick_upper is new create.
            let (fee_growth_inside_delta_0, fee_growth_inside_delta_1) = fee_growth_inside_delta_when_price_move(
                1000,
                0,
                500,
                0,
                -11,
                build_tick_with_fee_reward_growth(-10, 100, 0, 0).get_mut(),
                build_tick_with_fee_reward_growth(10, 0, 0, 0).get_mut(),
                true,
            );
            assert_eq!(fee_growth_inside_delta_0, 500);
            assert_eq!(fee_growth_inside_delta_1, 0);

            // tick_lower is new create with fee_growth_global_0_x64(1000)  and tick_upper is initialized with fee_growth_outside_0_x64(100)
            let (fee_growth_inside_delta_0, fee_growth_inside_delta_1) = fee_growth_inside_delta_when_price_move(
                1000,
                0,
                500,
                0,
                -11,
                build_tick_with_fee_reward_growth(-10, 1000, 0, 0).get_mut(),
                build_tick_with_fee_reward_growth(10, 100, 0, 0).get_mut(),
                true,
            );
            assert_eq!(fee_growth_inside_delta_0, 500);
            assert_eq!(fee_growth_inside_delta_1, 0);

            // tick_lower is initialized with fee_growth_outside_0_x64(50)  and tick_upper is initialized with fee_growth_outside_0_x64(100)
            let (fee_growth_inside_delta_0, fee_growth_inside_delta_1) = fee_growth_inside_delta_when_price_move(
                1000,
                0,
                500,
                0,
                -11,
                build_tick_with_fee_reward_growth(-10, 50, 0, 0).get_mut(),
                build_tick_with_fee_reward_growth(10, 100, 0, 0).get_mut(),
                true,
            );
            assert_eq!(fee_growth_inside_delta_0, 500);
            assert_eq!(fee_growth_inside_delta_1, 0);
        }

        #[test]
        fn price_in_tick_range_left_move_to_right_test() {
            // one_for_zero, price move to right and token_1 fee growth

            // tick_lower and tick_upper all new create
            let (fee_growth_inside_delta_0, fee_growth_inside_delta_1) = fee_growth_inside_delta_when_price_move(
                0,
                1000,
                500,
                -11,
                0,
                build_tick_with_fee_reward_growth(-10, 0, 0, 0).get_mut(),
                build_tick_with_fee_reward_growth(10, 0, 0, 0).get_mut(),
                true,
            );
            assert_eq!(fee_growth_inside_delta_0, 0);
            assert_eq!(fee_growth_inside_delta_1, 0);

            // tick_lower is initialized with fee_growth_outside_1_x64(100) and tick_upper is new create.
            let (fee_growth_inside_delta_0, fee_growth_inside_delta_1) = fee_growth_inside_delta_when_price_move(
                0,
                1000,
                500,
                -11,
                0,
                build_tick_with_fee_reward_growth(-10, 0, 100, 0).get_mut(),
                build_tick_with_fee_reward_growth(10, 0, 0, 0).get_mut(),
                true,
            );
            assert_eq!(fee_growth_inside_delta_0, 0);
            assert_eq!(fee_growth_inside_delta_1, 0);

            // tick_lower is new create  and tick_upper is initialized with fee_growth_outside_1_x64(100)
            let (fee_growth_inside_delta_0, fee_growth_inside_delta_1) = fee_growth_inside_delta_when_price_move(
                0,
                1000,
                500,
                -11,
                0,
                build_tick_with_fee_reward_growth(-10, 0, 0, 0).get_mut(),
                build_tick_with_fee_reward_growth(10, 0, 100, 0).get_mut(),
                true,
            );
            assert_eq!(fee_growth_inside_delta_0, 0);
            assert_eq!(fee_growth_inside_delta_1, 0);

            // tick_lower is initialized with fee_growth_outside_1_x64(50)  and tick_upper is initialized with fee_growth_outside_1_x64(100)
            let (fee_growth_inside_delta_0, fee_growth_inside_delta_1) = fee_growth_inside_delta_when_price_move(
                0,
                1000,
                500,
                -11,
                0,
                build_tick_with_fee_reward_growth(-10, 0, 50, 0).get_mut(),
                build_tick_with_fee_reward_growth(10, 0, 100, 0).get_mut(),
                true,
            );
            assert_eq!(fee_growth_inside_delta_0, 0);
            assert_eq!(fee_growth_inside_delta_1, 0);
        }

        #[test]
        fn price_in_tick_range_right_move_to_left_test() {
            // zero_for_one, price move to left and token_0 fee growth

            // tick_lower and tick_upper all new create
            let (fee_growth_inside_delta_0, fee_growth_inside_delta_1) = fee_growth_inside_delta_when_price_move(
                1000,
                0,
                500,
                11,
                0,
                build_tick_with_fee_reward_growth(-10, 1000, 0, 0).get_mut(),
                build_tick_with_fee_reward_growth(10, 1000, 0, 0).get_mut(),
                false,
            );
            assert_eq!(fee_growth_inside_delta_0, 0);
            assert_eq!(fee_growth_inside_delta_1, 0);

            // tick_lower is initialized with fee_growth_outside_0_x64(100) and tick_upper is new create.
            let (fee_growth_inside_delta_0, fee_growth_inside_delta_1) = fee_growth_inside_delta_when_price_move(
                1000,
                0,
                500,
                11,
                0,
                build_tick_with_fee_reward_growth(-10, 100, 0, 0).get_mut(),
                build_tick_with_fee_reward_growth(10, 1000, 0, 0).get_mut(),
                false,
            );
            assert_eq!(fee_growth_inside_delta_0, 0);
            assert_eq!(fee_growth_inside_delta_1, 0);

            // tick_lower is new create with fee_growth_global_0_x64(1000)  and tick_upper is initialized with fee_growth_outside_0_x64(100)
            let (fee_growth_inside_delta_0, fee_growth_inside_delta_1) = fee_growth_inside_delta_when_price_move(
                1000,
                0,
                500,
                11,
                0,
                build_tick_with_fee_reward_growth(-10, 1000, 0, 0).get_mut(),
                build_tick_with_fee_reward_growth(10, 100, 0, 0).get_mut(),
                false,
            );
            assert_eq!(fee_growth_inside_delta_0, 0);
            assert_eq!(fee_growth_inside_delta_1, 0);

            // tick_lower is initialized with fee_growth_outside_0_x64(50)  and tick_upper is initialized with fee_growth_outside_0_x64(100)
            let (fee_growth_inside_delta_0, fee_growth_inside_delta_1) = fee_growth_inside_delta_when_price_move(
                1000,
                0,
                500,
                11,
                0,
                build_tick_with_fee_reward_growth(-10, 50, 0, 0).get_mut(),
                build_tick_with_fee_reward_growth(10, 100, 0, 0).get_mut(),
                false,
            );
            assert_eq!(fee_growth_inside_delta_0, 0);
            assert_eq!(fee_growth_inside_delta_1, 0);
        }
    }

    mod get_reward_growths_inside_test {
        use super::*;
        use crate::states::*;
        use anchor_lang::prelude::Pubkey;

        fn build_reward_infos(reward_growth_global_x64: u128) -> [RewardInfo; 3] {
            [
                RewardInfo {
                    token_mint: Pubkey::new_unique(),
                    reward_growth_global_x64,
                    ..Default::default()
                },
                RewardInfo::default(),
                RewardInfo::default(),
            ]
        }

        fn reward_growth_inside_delta_when_price_move(
            init_reward_growth_global_x64: u128,
            reward_growth_global_delta: u128,
            mut tick_current: i32,
            target_tick_current: i32,
            tick_lower: &mut TickState,
            tick_upper: &mut TickState,
            cross_tick_lower: bool,
        ) -> u128 {
            let mut reward_growth_global_x64 = init_reward_growth_global_x64;
            let reward_growth_inside_before = TickUtils::get_reward_growths_inside(
                tick_lower,
                tick_upper,
                tick_current,
                &build_reward_infos(reward_growth_global_x64),
            )[0];

            reward_growth_global_x64 = reward_growth_global_x64 + reward_growth_global_delta;
            if cross_tick_lower {
                tick_lower.cross(0, 0, &build_reward_infos(reward_growth_global_x64));
            } else {
                tick_upper.cross(0, 0, &build_reward_infos(reward_growth_global_x64));
            }

            tick_current = target_tick_current;
            let reward_growth_inside_after = TickUtils::get_reward_growths_inside(
                tick_lower,
                tick_upper,
                tick_current,
                &build_reward_infos(reward_growth_global_x64),
            )[0];

            println!(
                "inside_delta:{}, reward_growth_inside_after:{}, reward_growth_inside_before:{}",
                reward_growth_inside_after.wrapping_sub(reward_growth_inside_before),
                reward_growth_inside_after,
                reward_growth_inside_before,
            );

            reward_growth_inside_after.wrapping_sub(reward_growth_inside_before)
        }

        #[test]
        fn uninitialized_reward_index_test() {
            let tick_current = 0;

            let tick_lower = &mut TickState {
                tick: -10,
                reward_growths_outside_x64: [1000, 0, 0],
                ..Default::default()
            };
            let tick_upper = &mut TickState {
                tick: 10,
                reward_growths_outside_x64: [1000, 0, 0],
                ..Default::default()
            };

            let reward_infos = &[RewardInfo::default(); 3];
            let reward_inside =
                TickUtils::get_reward_growths_inside(tick_lower, tick_upper, tick_current, reward_infos);
            assert_eq!(reward_inside, [0; 3]);
        }

        #[test]
        fn price_in_tick_range_move_to_right_test() {
            // tick_lower and tick_upper all new create
            let reward_frowth_inside_delta = reward_growth_inside_delta_when_price_move(
                1000,
                500,
                0,
                11,
                build_tick_with_fee_reward_growth(-10, 0, 0, 1000).get_mut(),
                build_tick_with_fee_reward_growth(10, 0, 0, 0).get_mut(),
                false,
            );
            assert_eq!(reward_frowth_inside_delta, 500);

            // tick_lower is initialized with reward_growths_outside_x64(100) and tick_upper is new create.
            let reward_frowth_inside_delta = reward_growth_inside_delta_when_price_move(
                1000,
                500,
                0,
                11,
                build_tick_with_fee_reward_growth(-10, 0, 0, 100).get_mut(),
                build_tick_with_fee_reward_growth(10, 0, 0, 0).get_mut(),
                false,
            );
            assert_eq!(reward_frowth_inside_delta, 500);

            // tick_lower is new create with reward_growths_outside_x64(1000)  and tick_upper is initialized with reward_growths_outside_x64(100)
            let reward_frowth_inside_delta = reward_growth_inside_delta_when_price_move(
                1000,
                500,
                0,
                11,
                build_tick_with_fee_reward_growth(-10, 0, 0, 1000).get_mut(),
                build_tick_with_fee_reward_growth(10, 0, 0, 100).get_mut(),
                false,
            );
            assert_eq!(reward_frowth_inside_delta, 500);

            // tick_lower is initialized with reward_growths_outside_x64(50)  and tick_upper is initialized with reward_growths_outside_x64(100)
            let reward_frowth_inside_delta = reward_growth_inside_delta_when_price_move(
                1000,
                500,
                0,
                11,
                build_tick_with_fee_reward_growth(-10, 0, 0, 50).get_mut(),
                build_tick_with_fee_reward_growth(10, 0, 0, 100).get_mut(),
                false,
            );
            assert_eq!(reward_frowth_inside_delta, 500);
        }

        #[test]
        fn price_in_tick_range_move_to_left_test() {
            // zero_for_one, cross tick_lower

            // tick_lower and tick_upper all new create, and tick_lower initialize with reward_growths_outside_x64(1000)
            let reward_frowth_inside_delta = reward_growth_inside_delta_when_price_move(
                1000,
                500,
                0,
                -11,
                build_tick_with_fee_reward_growth(-10, 0, 0, 1000).get_mut(),
                build_tick_with_fee_reward_growth(10, 0, 0, 0).get_mut(),
                true,
            );
            assert_eq!(reward_frowth_inside_delta, 500);

            // tick_lower is initialized with reward_growths_outside_x64(100) and tick_upper is new create.
            let reward_frowth_inside_delta = reward_growth_inside_delta_when_price_move(
                1000,
                500,
                0,
                -11,
                build_tick_with_fee_reward_growth(-10, 0, 0, 100).get_mut(),
                build_tick_with_fee_reward_growth(10, 0, 0, 0).get_mut(),
                true,
            );
            assert_eq!(reward_frowth_inside_delta, 500);

            // tick_lower is new create with reward_growths_outside_x64(1000)  and tick_upper is initialized with reward_growths_outside_x64(100)
            let reward_frowth_inside_delta = reward_growth_inside_delta_when_price_move(
                1000,
                500,
                0,
                -11,
                build_tick_with_fee_reward_growth(-10, 0, 0, 1000).get_mut(),
                build_tick_with_fee_reward_growth(10, 0, 0, 100).get_mut(),
                true,
            );
            assert_eq!(reward_frowth_inside_delta, 500);

            // tick_lower is initialized with reward_growths_outside_x64(50)  and tick_upper is initialized with reward_growths_outside_x64(100)
            let reward_frowth_inside_delta = reward_growth_inside_delta_when_price_move(
                1000,
                500,
                0,
                -11,
                build_tick_with_fee_reward_growth(-10, 0, 0, 50).get_mut(),
                build_tick_with_fee_reward_growth(10, 0, 0, 100).get_mut(),
                true,
            );
            assert_eq!(reward_frowth_inside_delta, 500);
        }
    }
    mod tick_array_layout_test {
        use crate::states::REWARD_NUM;

        use super::*;
        use anchor_lang::Discriminator;

        #[test]
        fn dyn_test_tick_array_layout() {
            let pool_id = Pubkey::new_unique();
            let start_tick_index: i32 = 60;
            let tick_spacing: u16 = 1;
            let mut padding: [u8; 107] = [0u8; 107];
            let mut padding_data = [0u8; 107];
            for i in 0..107 {
                padding[i] = i as u8;
                padding_data[i] = i as u8;
            }

            let liquidity_net: i128 = 0x11002233445566778899aabbccddeeff;
            let liquidity_gross: u128 = 0x11220033445566778899aabbccddeeff;
            let fee_growth_outside_0_x64: u128 = 0x11223300445566778899aabbccddeeff;
            let fee_growth_outside_1_x64: u128 = 0x11223344005566778899aabbccddeeff;
            let reward_growths_outside_x64: [u128; REWARD_NUM] = [
                0x11223344550066778899aabbccddeeff,
                0x11223344556600778899aabbccddeeff,
                0x11223344556677008899aabbccddeeff,
            ];
            let mut tick_padding: [u32; 13] = [0u32; 13];
            let mut tick_padding_data = [0u8; 4 * 13];
            let mut offset = 0;
            for i in 0..13 {
                tick_padding[i] = u32::MAX - 3 * i as u32;
                tick_padding_data[offset..offset + 4].copy_from_slice(&tick_padding[i].to_le_bytes());
                offset += 4;
            }

            let use_tick_index = start_tick_index + 2;
            let mut tick_state_item = TickState::default();
            tick_state_item.tick = use_tick_index;
            tick_state_item.liquidity_net = liquidity_net;
            tick_state_item.liquidity_gross = liquidity_gross;
            tick_state_item.fee_growth_outside_0_x64 = fee_growth_outside_0_x64;
            tick_state_item.fee_growth_outside_1_x64 = fee_growth_outside_1_x64;
            tick_state_item.reward_growths_outside_x64 = reward_growths_outside_x64;

            // 可以存下已经全部60个 tick-state 的内存空间
            // build tick data byte array
            let mut dyn_tick_array_full_account_data =
                [0u8; DynTickArrayState::HEADER_LEN + (TICK_ARRAY_SIZE as usize) * TickState::LEN];

            // write discriminator
            dyn_tick_array_full_account_data[..8].copy_from_slice(&DynTickArrayState::DISCRIMINATOR);

            let data = RefCell::new(&mut dyn_tick_array_full_account_data[..]);

            // 首次进行反序列化，并对header做初始化, 添加1个tick-state数据
            {
                let (mut header, mut ticks) = RefMut::map_split(data.borrow_mut(), |data_slice| {
                    let (header_bytes, ticks_bytes) = data_slice.split_at_mut(DynTickArrayState::HEADER_LEN);

                    // 将字节切片转换为对应的可变结构体引用
                    let header: &mut DynTickArrayState = bytemuck::from_bytes_mut(header_bytes[8..].as_mut());

                    let ticks: &mut [TickState] = bytemuck::try_cast_slice_mut(ticks_bytes)
                        .expect("Failed to cast ticks_bytes to TickState slice");

                    (header, ticks)
                });

                header.initialize(start_tick_index, tick_spacing, pool_id).unwrap();
                assert!(header.alloc_tick_count == 0);

                let _ = header.use_one_tick(use_tick_index, tick_spacing);
                ticks[0] = tick_state_item;
            }

            // 再对数据进行反序列化，应该已经有1个tick-state数据了
            {
                let (header, ticks) = RefMut::map_split(data.borrow_mut(), |data_slice| {
                    let (header_bytes, ticks_bytes) = data_slice.split_at_mut(DynTickArrayState::HEADER_LEN);

                    // 将字节切片转换为对应的可变结构体引用
                    let header: &mut DynTickArrayState = bytemuck::from_bytes_mut(header_bytes[8..].as_mut());

                    let ticks: &mut [TickState] = bytemuck::try_cast_slice_mut(ticks_bytes)
                        .expect("Failed to cast ticks_bytes to TickState slice");

                    (header, ticks)
                });

                assert!(header.alloc_tick_count == 1);
                assert!(header.start_tick_index == start_tick_index);
                assert!(header.pool_id == pool_id);

                let tick_state = &ticks[0];

                assert!(tick_state.tick == use_tick_index);
                assert!(tick_state.liquidity_net == liquidity_net);
                assert!(tick_state.liquidity_gross == liquidity_gross);
                assert!(tick_state.fee_growth_outside_0_x64 == fee_growth_outside_0_x64);
                assert!(tick_state.fee_growth_outside_1_x64 == fee_growth_outside_1_x64);
            }
        }
    }
}
