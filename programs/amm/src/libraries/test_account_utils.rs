// 仅在测试时使用的utils
#[cfg(test)]
use anchor_lang::prelude::*;
#[cfg(test)]
use anchor_lang::ZeroCopy;
#[cfg(test)]
use std::cell::RefCell;
#[cfg(test)]
use std::rc::Rc;

/// only for test
/// mock 一个 AccountInfo
#[cfg(test)]
pub fn mock_account_info<'a>(
    key: &'a Pubkey,
    owner: &'a Pubkey,
    is_signer: bool,
    is_writable: bool,
    lamports: u64,
    data_len: usize,
) -> (AccountInfo<'a>, Rc<RefCell<&'a mut u64>>, Rc<RefCell<&'a mut [u8]>>) {
    // 关键点：lamports 和 data 必须由测试持有所有权，以保证 &'static mut 引用有效期
    let lamports_box = Box::new(lamports);
    let data_vec = vec![0u8; data_len].into_boxed_slice();

    // 将 Box 转换为原始指针再转 &'static mut，测试中用是安全的，因为我们持有 Box 的生命周期
    let lamports_ptr: *mut u64 = Box::into_raw(lamports_box);
    let data_ptr: *mut [u8] = Box::into_raw(data_vec);

    // 将原始指针转回 Box，保持所有权，以便函数返回后不泄漏
    // 注意：我们需要把 &'static mut 借用交给 AccountInfo，但也要把 Box 返回给调用者保存，避免提前 drop
    let lamports_owner = unsafe { Box::from_raw(lamports_ptr) };
    let data_owner = unsafe { Box::from_raw(data_ptr) };

    // 再次取得可变引用的 'static 生命周期
    let lamports_ref: &'static mut u64 = unsafe { &mut *(Box::into_raw(lamports_owner)) };
    let data_ref: &'static mut [u8] = unsafe { &mut *(Box::into_raw(data_owner)) };

    // 将引用包装在 Rc<RefCell<>> 中以符合 AccountInfo 的要求
    let lamports_rc = Rc::new(RefCell::new(lamports_ref));
    let data_rc = Rc::new(RefCell::new(data_ref));

    let account_info = AccountInfo {
        key: key,
        is_signer,
        is_writable,
        lamports: lamports_rc.clone(),
        data: data_rc.clone(),
        owner: owner,
        executable: false,
        rent_epoch: 0,
    };

    (account_info, lamports_rc, data_rc)
}

/// only for test
#[cfg(test)]
pub fn mock_anchor_account_info<'a, 'b, T: ZeroCopy>(
    key: &'a Pubkey,
    owner: &'a Pubkey,
    is_signer: bool,
    is_writable: bool,
    lamports: u64,
    account: &'b T,
) -> (AccountInfo<'a>, Rc<RefCell<&'a mut u64>>, Rc<RefCell<&'a mut [u8]>>) {
    // 计算 data 长度：8 字节 discriminator + 序列化数据
    let mut buf = Vec::new();
    // 预留 discriminator + 内容
    buf.extend_from_slice(T::DISCRIMINATOR);
    buf.extend_from_slice(bytemuck::bytes_of(account));

    // 构造 AccountInfo
    let data_len = buf.len();
    let (ai, lamports_box, data_box) = mock_account_info(key, owner, is_signer, is_writable, lamports, data_len);

    data_box.borrow_mut().copy_from_slice(&buf);

    // 将 data_box 放回 ai 的 data 已经在 mock_account_info 内完成，这里只需要覆写内容
    (ai, lamports_box, data_box)
}

/// only for test
#[cfg(test)]
pub fn mock_anchor_account_info_v2<'a, 'b, T: ZeroCopy>(
    key: &'a Pubkey,
    owner: &'a Pubkey,
    is_signer: bool,
    is_writable: bool,
    lamports: u64,
    account: &'b T,
    extra_account_data: Option<&[u8]>,
) -> (AccountInfo<'a>, Rc<RefCell<&'a mut u64>>, Rc<RefCell<&'a mut [u8]>>) {
    // 计算 data 长度：8 字节 discriminator + 序列化数据
    let mut buf = Vec::new();
    // 预留 discriminator + 内容
    buf.extend_from_slice(T::DISCRIMINATOR);
    buf.extend_from_slice(bytemuck::bytes_of(account));
    // 追加额外数据
    if let Some(extra_account_data) = extra_account_data {
        buf.extend_from_slice(extra_account_data);
    }

    // 构造 AccountInfo
    let data_len = buf.len();
    let (ai, lamports_box, data_box) = mock_account_info(key, owner, is_signer, is_writable, lamports, data_len);

    data_box.borrow_mut().copy_from_slice(&buf);

    // 将 data_box 放回 ai 的 data 已经在 mock_account_info 内完成，这里只需要覆写内容
    (ai, lamports_box, data_box)
}

#[cfg(test)]
pub fn mock_anchor_account_info_v3<'a, 'b, T: ZeroCopy>(
    key: &'a Pubkey,
    owner: &'a Pubkey,
    account: &'b T,
    extra_account_data: Option<&[u8]>,
) -> (AccountInfo<'a>, Rc<RefCell<&'a mut u64>>, Rc<RefCell<&'a mut [u8]>>) {
    mock_anchor_account_info_v2(key, owner, false, true, 0, account, extra_account_data)
}
