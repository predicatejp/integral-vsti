// Copyright (C) 2026 Yusuke Oda
// SPDX-License-Identifier: AGPL-3.0-only

use std::{collections::VecDeque, sync::Mutex};

pub(crate) enum Edit {
    /// 選択リストの単発編集。
    Set(usize, f64),
    /// ノブのドラッグ開始。
    Begin(usize),
    /// ドラッグ中の正規化値。
    Value(f64),
    /// 最後の値も渡し、満杯で落ちた更新を確定する。
    End(Option<f64>),
}

struct EditState {
    /// UIスレッド間だけで使う32件の通知。末尾1件は終了用に予約。
    pending: VecDeque<Edit>,
}

pub(crate) struct Edits {
    /// ホストを呼ぶ前に必ずロックを解放する。音声スレッドは参照しない。
    state: Mutex<EditState>,
}

impl Edits {
    pub(crate) fn new() -> Self {
        Self {
            state: Mutex::new(EditState {
                pending: VecDeque::with_capacity(32),
            }),
        }
    }

    pub(crate) fn push(&self, edit: Edit) -> bool {
        let mut state = self.state.lock().unwrap();
        let limit = if matches!(edit, Edit::End(_)) { 32 } else { 31 };
        if state.pending.len() >= limit {
            return false;
        }
        state.pending.push_back(edit);
        true
    }

    pub(crate) fn pop(&self) -> Option<Edit> {
        self.state.lock().unwrap().pending.pop_front()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn full_queue_keeps_end_and_final_value() {
        let edits = Edits::new();
        assert!(edits.push(Edit::Begin(1)));
        for _ in 0..30 {
            assert!(edits.push(Edit::Value(0.5)));
        }
        assert!(!edits.push(Edit::Value(0.75)));
        assert!(edits.push(Edit::End(Some(0.75))));
        assert!(!edits.push(Edit::Begin(2)));
        assert!(matches!(edits.pop(), Some(Edit::Begin(1))));
        for _ in 0..30 {
            assert!(matches!(edits.pop(), Some(Edit::Value(0.5))));
        }
        assert!(matches!(edits.pop(), Some(Edit::End(Some(0.75)))));
        assert!(edits.pop().is_none());
    }
}
