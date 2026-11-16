// Copyright (C) 2026 Yusuke Oda
// SPDX-License-Identifier: AGPL-3.0-only

//! processの借用期間内でホストのパラメーター変更キューを読む。

use super::{
    COUNT, HOST_COUNT, PITCH_BEND_ID, PITCH_BEND_INDEX, PROGRAM_ID, PROGRAM_INDEX, canonical,
    holds_between_points, index,
};
use crate::abi::*;
use std::ptr;

/// 1回のprocess中だけホストから借用する、1パラメーターの変更点のカーソル。
pub struct ParameterPoints<'a> {
    /// ホスト所有のIParamValueQueue。利用可能なキューがなければnull。
    queue: This,
    /// dataと同じ呼び出し中だけ有効な関数表。所有参照を取得しない。
    table: Option<&'a ParamValueQueueVTable>,
    /// キューが申告した変更点数。
    count: i32,
    /// 次に読み出す変更点の列挙位置。
    index: i32,
    /// 最後に受理したoffset。後戻りする変更点を除外するために保持する。
    previous_offset: i32,
    /// 今回のブロック長。0ではoffset 0だけを受理する。
    samples: i32,
    /// 次に適用する変更点。同じoffsetの変更はキュー順に適用する。
    next: Option<(i32, f64)>,
    /// 補間の始点。前ブロックの最終値をoffset=-1として扱う。
    previous: (i32, f64),
    /// holds_between_pointsの指定値は変更点まで保持し、それ以外は正規化空間で線形補間する。
    parameter: usize,
    /// キュー作成時に確定する補間モード。サンプルごとのParamID分類を省く。
    interpolates: bool,
    /// 有効な変更点があるキューだけをDSPから共有値へ書き戻す。
    /// 他パラメーターの状態復元を、無関係なオートメーションで上書きしない。
    pub automated: bool,
}

impl<'a> ParameterPoints<'a> {
    /// ホストのキューを1回列挙し、ParamIDごとのカーソルを構成する。
    /// SAFETY: dataの入れ子のインターフェースはホスト所有で、dataの借用期間中有効。
    pub unsafe fn all(
        data: &'a ProcessData,
        values: &[f64; COUNT],
        pitch_bend: f64,
    ) -> [Self; HOST_COUNT] {
        let mut points = std::array::from_fn(|i| {
            Self::empty(
                data.num_samples,
                i,
                values.get(i).copied().unwrap_or(if i == PITCH_BEND_INDEX {
                    pitch_bend
                } else {
                    0.0
                }),
            )
        });
        if !data.input_parameter_changes.is_null() {
            // SAFETY: ホストのIParameterChangesと借用キューを読み、process中だけ保持する。
            unsafe {
                let changes = data.input_parameter_changes;
                let table = &**changes.cast::<*const ParameterChangesVTable>();
                for at in 0..(table.get_parameter_count)(changes) {
                    let queue = (table.get_parameter_data)(changes, at);
                    if queue.is_null() {
                        continue;
                    }
                    let queue_table = &**queue.cast::<*const ParamValueQueueVTable>();
                    let id = (queue_table.get_parameter_id)(queue);
                    let parameter = if id == PROGRAM_ID {
                        Some(PROGRAM_INDEX)
                    } else if id == PITCH_BEND_ID {
                        Some(PITCH_BEND_INDEX)
                    } else {
                        index(id)
                    };
                    let Some(parameter) = parameter else {
                        continue;
                    };
                    let point = &mut points[parameter];
                    if point.table.is_some() {
                        continue; // ParamID重複時は最初のキューを採用する。
                    }
                    point.queue = queue;
                    point.table = Some(queue_table);
                    point.count = (queue_table.get_point_count)(queue);
                    point.advance();
                    point.automated = point.next.is_some();
                }
            }
        }
        points
    }

    fn empty(samples: i32, parameter: usize, previous_value: f64) -> Self {
        Self {
            queue: ptr::null_mut(),
            table: None,
            count: 0,
            index: 0,
            previous_offset: 0,
            samples,
            next: None,
            previous: (-1, previous_value),
            parameter,
            interpolates: !holds_between_points(parameter),
            automated: false,
        }
    }

    /// Programは同じ番号の再選択もロード要求として扱う。
    pub fn take_program_at(&mut self, offset: i32) -> bool {
        let changed = self.next.is_some_and(|(at, _)| at <= offset);
        self.value_at(offset);
        changed
    }

    /// プログラム切替点を次の補間の始点とする。同位置の明示的な変更点を優先する。
    pub fn reset_value(&mut self, offset: i32, value: f64) {
        self.previous = (offset, value);
    }

    pub fn value_at(&mut self, offset: i32) -> f64 {
        while let Some((at, value)) = self.next {
            if at > offset {
                break;
            }
            self.previous = (at, value);
            self.advance();
        }
        if self.interpolates
            && let Some((at, value)) = self.next
        {
            let fraction = f64::from(offset - self.previous.0) / f64::from(at - self.previous.0);
            return self.previous.1 + (value - self.previous.1) * fraction;
        }
        self.previous.1
    }

    pub fn advance(&mut self) {
        self.next = None;
        let Some(table) = self.table else {
            return;
        };
        while self.index < self.count {
            let mut offset = -1;
            let mut value = f64::NAN;
            // SAFETY: allの契約によりキューは生存中。出力先は型の一致したローカル変数。
            let status =
                unsafe { (table.get_point)(self.queue, self.index, &mut offset, &mut value) };
            self.index += 1;
            if status != OK
                || offset < self.previous_offset
                || (self.samples == 0 && offset != 0)
                || (self.samples > 0 && offset >= self.samples)
            {
                continue;
            }
            if (0.0..=1.0).contains(&value) {
                self.previous_offset = offset;
                self.next = Some((offset, canonical(self.parameter, value)));
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parameters::*;

    #[test]
    fn sample_positions_interpolate_only_known_continuous_intervals() {
        let mut points = ParameterPoints::empty(8, 32, 0.2);
        points.previous = (0, 0.2);
        points.next = Some((4, 1.0));
        for (at, expected) in [(0, 0.2), (1, 0.4), (2, 0.6), (3, 0.8)] {
            assert!((points.value_at(at) - expected).abs() < 1e-15);
        }
        points.next = None;
        assert_eq!(points.value_at(1), 0.2);
        points.next = Some((4, 1.0));
        points.interpolates = false;
        assert_eq!(points.value_at(1), 0.2);
    }

    #[test]
    fn velocity_sensitivity_accepts_percentages_and_linear_automation() {
        for index in [130, 131] {
            for plain in [0.0, 37.25, 100.0] {
                assert!(valid_plain(index, plain));
                assert!((to_plain(index, to_normalized(index, plain)) - plain).abs() < 1e-12);
            }
            for plain in [-0.1, 100.1, f64::NAN, f64::INFINITY] {
                assert!(!valid_plain(index, plain));
            }
            let mut points = ParameterPoints::empty(8, index, 1.0);
            points.next = Some((3, 0.0));
            for expected in [0.75, 0.5, 0.25, 0.0, 0.0, 0.0, 0.0, 0.0]
                .into_iter()
                .enumerate()
            {
                assert_eq!(points.value_at(expected.0 as i32), expected.1);
            }
        }
    }

    #[test]
    fn delay_stereo_switches_at_automation_point_in_each_slot() {
        for (slot, indices) in EFFECT_SLOTS.iter().enumerate() {
            let index = indices[7];
            assert_eq!(choices(index), ["Dual", "Cross", "PP-L", "PP-R"]);
            let mut values = defaults();
            for (mode, expected) in DELAY_STEREO_MODES.into_iter().enumerate() {
                let value = to_normalized(index, mode as f64);
                values[index] = value;
                assert_eq!(delay(&values, slot).stereo, expected);
                let mut points = ParameterPoints::empty(8, index, 0.0);
                points.next = Some((4, value));
                for sample in 0..4 {
                    assert_eq!(points.value_at(sample), 0.0);
                }
                for sample in 4..8 {
                    assert_eq!(points.value_at(sample), value);
                }
            }
        }
    }
}
