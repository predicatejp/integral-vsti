// Copyright (C) 2026 Yusuke Oda
// SPDX-License-Identifier: AGPL-3.0-only

//! 環境変数で明示的に有効化するホスト接続診断。
//! 音声処理側はatomicの更新だけを行い、整形・ファイル出力は処理停止後のインスタンス終了時に行う。

#[cfg(not(miri))]
use std::sync::atomic::AtomicU64;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};

/// 診断項目の配列位置。値の名前はsnapshotで同じ順序に対応させる。
#[derive(Clone, Copy)]
#[repr(usize)]
pub enum Metric {
    Initialize,
    Setup,
    SetActive,
    SetProcessing,
    AudioActivation,
    EventActivation,
    Process,
    NotProcessing,
    InvalidFormat,
    InvalidBuffer,
    NoOutput,
    InactiveOutput,
    InactiveEvents,
    NoEventList,
    EventCount,
    EventReadFailed,
    EventRejected,
    NoteOn,
    NoteOff,
    NoteOnRejected,
    NonzeroOutput,
    Samples,
    Inputs,
    Outputs,
    Precision,
    MaxSamples,
    SampleRateBits,
    Active,
    Processing,
    AudioActive,
    EventActive,
    EventType,
    EventBus,
    EventOffset,
    NoteChannel,
    NotePitch,
    NoteId,
    VelocityBits,
    TuningBits,
}
const NAMES: [&str; 39] = [
    "initialize_calls",
    "setup_calls",
    "set_active_calls",
    "set_processing_calls",
    "audio_activation_calls",
    "event_activation_calls",
    "process_calls",
    "not_processing_calls",
    "invalid_format_calls",
    "invalid_buffer_calls",
    "no_output_calls",
    "inactive_output_calls",
    "inactive_event_calls",
    "no_event_list_calls",
    "events_reported",
    "event_read_failures",
    "events_rejected",
    "note_on_events",
    "note_off_events",
    "note_on_rejected",
    "nonzero_output_blocks",
    "last_num_samples",
    "last_num_inputs",
    "last_num_outputs",
    "last_sample_precision",
    "last_setup_max_samples",
    "last_setup_sample_rate_f64_bits",
    "last_active_request",
    "last_processing_request",
    "audio_active",
    "event_active",
    "last_event_type",
    "last_event_bus",
    "last_event_offset",
    "last_note_channel",
    "last_note_pitch",
    "last_note_id",
    "last_velocity_f32_bits",
    "last_tuning_f32_bits",
];

struct Counters {
    /// 列挙MetricとNAMESに対応する回数・直近値。音声スレッドからはロックなしで更新する。
    values: [AtomicI64; NAMES.len()],
    /// インスタンスの終了時にtrueとする。診断レポートが終了時の記録であることを示す。
    finished: AtomicBool,
}

impl Counters {
    fn new() -> Self {
        Self {
            values: std::array::from_fn(|_| AtomicI64::new(0)),
            finished: AtomicBool::new(false),
        }
    }

    fn snapshot(&self) -> String {
        let mut text = format!(
            "version={}\nfinished={}\n",
            env!("SAMPLE_VSTI_VERSION"),
            self.finished.load(Ordering::Acquire)
        );
        for (name, value) in NAMES.iter().zip(&self.values) {
            use std::fmt::Write;
            let _ = writeln!(text, "{name}={}", value.load(Ordering::Relaxed));
        }
        text
    }
}

struct Session {
    /// オーディオ処理が更新する回数・直近値。
    counters: Counters,
    /// インスタンス生成時に決定する出力先。処理中にはアクセスしない。
    path: std::path::PathBuf,
}

pub struct Diagnostics {
    /// 明示的に有効化した場合だけ所有する診断セッション。通常実行ではNone。
    session: Option<Box<Session>>,
}

impl Diagnostics {
    /// インスタンス生成時にだけ環境変数を読む。指定先は既存ディレクトリに限定する。
    pub fn new() -> Self {
        // Miriではホスト環境へのアクセスを行わない。
        #[cfg(miri)]
        return Self { session: None };
        #[cfg(not(miri))]
        {
            static NEXT_ID: AtomicU64 = AtomicU64::new(0);
            let session = std::env::var_os("SAMPLE_VSTI_DIAGNOSTICS_DIR")
                .map(std::path::PathBuf::from)
                .filter(|directory| directory.is_dir())
                .map(|directory| {
                    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
                    Box::new(Session {
                        counters: Counters::new(),
                        path: directory
                            .join(format!("sample-vsti-{}-{id}.txt", std::process::id())),
                    })
                });
            Self { session }
        }
    }

    pub fn add(&self, metric: Metric, value: i64) {
        if let Some(session) = &self.session {
            session.counters.values[metric as usize].fetch_add(value, Ordering::Relaxed);
        }
    }

    pub fn set(&self, metric: Metric, value: i64) {
        if let Some(session) = &self.session {
            session.counters.values[metric as usize].store(value, Ordering::Relaxed);
        }
    }

    /// ホストのterminate（UIスレッド、処理停止後）からだけ呼ぶ。process/setProcessingからは呼ばない。
    pub fn finish(&self) {
        if let Some(session) = &self.session {
            session.counters.finished.store(true, Ordering::Release);
            // 診断ファイルが書けなくてもプラグインの終了を失敗させない。
            let _ = std::fs::write(&session.path, session.counters.snapshot());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counters_report_blocked_processing_and_note_values() {
        let diagnostics = Diagnostics {
            session: Some(Box::new(Session {
                counters: Counters::new(),
                path: std::path::PathBuf::new(),
            })),
        };
        diagnostics.add(Metric::Process, 2);
        diagnostics.add(Metric::InactiveEvents, 2);
        diagnostics.set(Metric::NotePitch, 69);
        diagnostics.set(Metric::EventOffset, -1);
        let snapshot = diagnostics.session.as_ref().unwrap().counters.snapshot();
        assert!(snapshot.contains("finished=false\n"));
        assert!(snapshot.contains("process_calls=2\n"));
        assert!(snapshot.contains("inactive_event_calls=2\n"));
        assert!(snapshot.contains("last_note_pitch=69\n"));
        assert!(snapshot.contains("last_event_offset=-1\n"));
    }
}
