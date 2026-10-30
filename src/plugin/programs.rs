// Copyright (C) 2026 Yusuke Oda
// SPDX-License-Identifier: AGPL-3.0-only

//! 単一ルートユニットと内蔵SineのVST3プログラム一覧。

use super::*;

pub(super) static UNIT_INFO: UnitInfoVTable = UnitInfoVTable {
    unknown: UNKNOWN,
    get_unit_count: count,
    get_unit_info: unit_info,
    get_program_list_count: count,
    get_program_list_info: list_info,
    get_program_name: program_name,
    get_program_info: program_info,
    has_program_pitch_names: has_pitch_names,
    get_program_pitch_name: pitch_name,
    get_selected_unit: selected_unit,
    select_unit,
    get_unit_by_bus: unit_by_bus,
    set_unit_program_data: set_program_data,
};

unsafe extern "system" fn count(_: This) -> i32 {
    1
}
unsafe extern "system" fn unit_info(_: This, index: i32, out: *mut UnitInfo) -> TResult {
    if index != 0 || out.is_null() {
        return INVALID_ARGUMENT;
    }
    // SAFETY: outはホストのUnitInfo書き込み領域。
    unsafe {
        out.write(UnitInfo {
            id: 0,
            parent_unit_id: -1,
            name: utf16("SampleVsti"),
            program_list_id: 0,
        })
    };
    OK
}
unsafe extern "system" fn list_info(_: This, index: i32, out: *mut ProgramListInfo) -> TResult {
    if index != 0 || out.is_null() {
        return INVALID_ARGUMENT;
    }
    // SAFETY: outはホストのProgramListInfo書き込み領域。
    unsafe {
        out.write(ProgramListInfo {
            id: 0,
            name: utf16("Factory"),
            program_count: 1,
        })
    };
    OK
}
pub(super) unsafe extern "system" fn program_name(
    _: This,
    list: i32,
    index: i32,
    out: *mut u16,
) -> TResult {
    if list != 0 || index != 0 || out.is_null() {
        return INVALID_ARGUMENT;
    }
    let name: [u16; 128] = utf16(parameters::PROGRAM_NAME);
    // SAFETY: outはホストのString128書き込み領域。
    unsafe { ptr::copy_nonoverlapping(name.as_ptr(), out, name.len()) };
    OK
}
unsafe extern "system" fn program_info(
    _: This,
    _: i32,
    _: i32,
    _: *const u8,
    _: *mut u16,
) -> TResult {
    FALSE
}
unsafe extern "system" fn has_pitch_names(_: This, _: i32, _: i32) -> TResult {
    FALSE
}
unsafe extern "system" fn pitch_name(_: This, _: i32, _: i32, _: i16, _: *mut u16) -> TResult {
    FALSE
}
unsafe extern "system" fn selected_unit(_: This) -> i32 {
    0
}
unsafe extern "system" fn select_unit(_: This, id: i32) -> TResult {
    if id == 0 { OK } else { INVALID_ARGUMENT }
}
unsafe extern "system" fn unit_by_bus(
    _: This,
    media: i32,
    direction: i32,
    bus: i32,
    channel: i32,
    out: *mut i32,
) -> TResult {
    if out.is_null() {
        return INVALID_ARGUMENT;
    }
    if bus != 0
        || !((media == EVENT && direction == INPUT && (0..16).contains(&channel))
            || (media == AUDIO && direction == OUTPUT && (0..2).contains(&channel)))
    {
        return FALSE;
    }
    // SAFETY: outはホストのUnitID書き込み領域。
    unsafe { out.write(0) };
    OK
}
unsafe extern "system" fn set_program_data(_: This, _: i32, _: i32, _: This) -> TResult {
    NOT_IMPLEMENTED
}
