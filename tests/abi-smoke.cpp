// Copyright (C) 2026 Yusuke Oda
// SPDX-License-Identifier: AGPL-3.0-only

// 検証専用。SDKのC++インターフェースからRust共有ライブラリを直接呼び出す。
// このコードとSDKはプラグイン本体にリンクしない。
#include "pluginterfaces/base/ipluginbase.h"
#include "pluginterfaces/base/ibstream.h"
#include "pluginterfaces/vst/ivstaudioprocessor.h"
#include "pluginterfaces/vst/ivsteditcontroller.h"
#include "pluginterfaces/vst/ivstevents.h"
#include "pluginterfaces/vst/ivstunits.h"
#include "pluginterfaces/vst/ivstparameterchanges.h"
#include "pluginterfaces/vst/vstspeaker.h"

#include <array>
#include <algorithm>
#include <cassert>
#include <cstddef>
#include <cstring>
#include <cmath>
#include <vector>
#include <utility>
#include <limits>
#ifdef _WIN32
#include <windows.h>
#else
#include <dlfcn.h>
#endif
#include <string>
#include <iostream>
#include <type_traits>

using namespace Steinberg;
using namespace Steinberg::Vst;

// Rust側のlayoutテストと対にし、公式ヘッダのレイアウトを独立して確認する。
static_assert(sizeof(PFactoryInfo) == 452);
static_assert(sizeof(PClassInfo) == 116);
static_assert(sizeof(PClassInfo2) == 440);
static_assert(sizeof(BusInfo) == 276);
static_assert(sizeof(ParameterInfo) == 792 && alignof(ParameterInfo) == 8);
static_assert(offsetof(ParameterInfo, defaultNormalizedValue) == 776);
static_assert(offsetof(ParameterInfo, flags) == 788);
static_assert(sizeof(UnitInfo) == 268 && alignof(UnitInfo) == 4);
static_assert(offsetof(UnitInfo, programListId) == 264);
static_assert(sizeof(ProgramListInfo) == 264 && offsetof(ProgramListInfo, programCount) == 260);
static_assert(sizeof(RoutingInfo) == 12);
static_assert(sizeof(ProcessSetup) == 24);
static_assert(offsetof(ProcessSetup, sampleRate) == 16);
static_assert(sizeof(AudioBusBuffers) == 24);
static_assert(offsetof(AudioBusBuffers, silenceFlags) == 8);
static_assert(offsetof(AudioBusBuffers, channelBuffers32) == 16);
static_assert(sizeof(ProcessData) == 80);
static_assert(alignof(ProcessData) == 8);
static_assert(offsetof(ProcessData, inputs) == 24);
static_assert(offsetof(ProcessData, processContext) == 72);

static_assert(sizeof(NoteOnEvent) == 20 && offsetof(NoteOnEvent, noteId) == 16);
static_assert(sizeof(NoteOffEvent) == 16 && offsetof(NoteOffEvent, noteId) == 8);
static_assert(sizeof(Event) == 48 && alignof(Event) == 8);
static_assert(offsetof(Event, type) == 18 && offsetof(Event, noteOn) == 24);
static_assert(sizeof(NoteExpressionTextEvent) == 24);

// ホスト側の独立したIEventList実装。借用中にretain/release/addEventされないことも検査。
class Events final : public IEventList {
public:
    std::vector<Event> values;
    int32 failedIndex = -1;
    tresult PLUGIN_API queryInterface(const TUID, void** out) override { *out = nullptr; return kNoInterface; }
    uint32 PLUGIN_API addRef() override { assert(false); return 1; }
    uint32 PLUGIN_API release() override { assert(false); return 1; }
    int32 PLUGIN_API getEventCount() override { return static_cast<int32>(values.size()); }
    tresult PLUGIN_API getEvent(int32 index, Event& event) override {
        if (index == failedIndex) return kResultFalse;
        event = values.at(static_cast<size_t>(index));
        return kResultOk;
    }
    tresult PLUGIN_API addEvent(Event&) override { assert(false); return kNotImplemented; }
};

Event note(bool on, int32 offset, int32 id, int16 pitch = 69, int16 channel = 0, float velocity = 1.0f) {
    Event event {};
    event.sampleOffset = offset;
    event.type = on ? Event::kNoteOnEvent : Event::kNoteOffEvent;
    if (on) event.noteOn = {channel, pitch, 0.0f, velocity, 0, id};
    else event.noteOff = {channel, pitch, 0.0f, id, 0.0f};
    return event;
}

void configureEnvelope(IAudioProcessor*, bool, double timeScale = 1.0);

template <typename Sample>
void checkNotes(IComponent* component, IAudioProcessor* processor, int32 precision, double rate, int32 mode,
                bool activateEvents) {
    configureEnvelope(processor, true); // 既存の波形・イベント検証は時間0、sustain=1で行う。
    ProcessSetup setup {mode, precision, 257, rate};
    assert(processor->setupProcessing(setup) == kResultOk);
    assert(processor->getTailSamples() == kInfiniteTail);
    assert(component->activateBus(kAudio, kOutput, 0, true) == kResultOk);
    // 有効化通知を省略するホストの経路でも、同じ波形・イベント時刻を検証する。
    if (activateEvents) assert(component->activateBus(kEvent, kInput, 0, true) == kResultOk);
    assert(component->setActive(true) == kResultOk);
    assert(processor->setProcessing(true) == kResultOk);
    std::array<Sample, 259> left, right;
    Sample* channels[] = {left.data() + 1, right.data() + 1};
    AudioBusBuffers bus;
    bus.numChannels = 2;
    if constexpr (std::is_same_v<Sample, float>) bus.channelBuffers32 = channels;
    else bus.channelBuffers64 = channels;
    Events events;
    ProcessData data;
    data.processMode = mode;
    data.symbolicSampleSize = precision;
    data.numOutputs = 1;
    data.outputs = &bus;
    data.inputEvents = &events;
    double phase = 0.0;
    // 期待する音程・振幅の区間をテスト側で明示する。位相は音が出るサンプルだけ進む。
    auto check = [&](int32 length, auto expected) {
        data.numSamples = length;
        left.fill(Sample(0.75)); right.fill(Sample(0.75));
        assert(processor->process(data) == kResultOk);
        bool nonzero = false;
        for (int32 i = 0; i < length; ++i) {
            const auto [frequency, amplitude] = expected(i);
            double sample = 0.0;
            if (frequency > 0.0) {
                sample = amplitude * std::sin(2.0 * std::acos(-1.0) * phase);
                phase = std::fmod(phase + frequency / rate, 1.0);
            }
            const auto rounded = Sample(sample);
            nonzero |= rounded != Sample(0);
            for (auto* channel : channels) {
                assert(std::isfinite(channel[i]));
                assert(std::abs(channel[i] - rounded) < 1e-6);
            }
        }
        assert(bus.silenceFlags == (nonzero ? 0u : 3u));
        for (auto* channel : channels) { assert(channel[-1] == Sample(0.75)); assert(channel[length] == Sample(0.75)); }
    };
    using Tone = std::array<double, 2>;
    auto silence = [](int32) { return Tone {0.0, 0.0}; };
    auto high = [](int32) { return Tone {880.0, 0.25 * std::sqrt(2.0)}; };
    auto low = [](int32) { return Tone {440.0, 0.5 * std::sqrt(2.0)}; };
    events.values = {note(true, 0, 10), note(true, 3, 11, 81, 0, 0.5f), note(false, 5, 10),
                     note(false, 7, 11, 81), note(true, 10, 12), note(false, 15, 12),
                     note(true, 16, 13, 81, 0, 0.5f)};
    check(17, [](int32 i) {
        if (i < 3 || (i >= 10 && i < 15)) return Tone {440.0, 0.5 * std::sqrt(2.0)};
        if ((i >= 3 && i < 7) || i == 16) return Tone {880.0, 0.25 * std::sqrt(2.0)};
        return Tone {0.0, 0.0};
    });
    data.inputEvents = nullptr;
    for (int32 length : {1, 7, 256, 257}) check(length, high);
    channels[1] = channels[0]; // 発音時の左右エイリアスでも位相を二重に進めない。
    check(7, high);
    channels[1] = right.data() + 1;
    data.inputEvents = &events;
    // 出力バッファのないゼロサンプルでもノートオフを処理する。
    events.values = {note(false, 0, 13, 81)};
    data.numSamples = 0; data.numOutputs = 0; data.outputs = nullptr;
    assert(processor->process(data) == kResultOk);
    data.numOutputs = 1; data.outputs = &bus;
    events.values.clear(); check(7, silence);
    // 同時刻はリスト順。IDなしはチャンネルと音高で照合する。
    events.values = {note(true, 0, -1, 69, 2), note(false, 0, -1, 69, 3), note(false, 0, -1, 70, 2)};
    check(7, low);
    events.values = {note(false, 0, -1, 69, 2), note(true, 0, 20)};
    check(7, low);
    // 不正・未対応イベントは現在の発音を壊さない。getEvent失敗も読み出さない。
    events.values = {note(false, 0, 20), note(false, -1, 20), note(false, 7, 20),
                     note(true, 0, 30, 128), note(true, 0, 30, 69, 16),
                     note(true, 0, 30, 69, 0, std::numeric_limits<float>::quiet_NaN()),
                     note(false, 0, 20), note(false, 0, 20)};
    events.failedIndex = 0;
    events.values[6].busIndex = 1;
    events.values[7].type = Event::kDataEvent;
    check(7, low);
    events.failedIndex = -1;
    // 後戻りするoffsetは無視する。
    events.values = {note(true, 3, 20), note(false, 2, 20)};
    check(7, low);
    // 非アクティブ出力でもノートオフを落とさない。
    assert(component->activateBus(kAudio, kOutput, 0, false) == kResultOk);
    events.values = {note(false, 0, 20)};
    bus.channelBuffers32 = nullptr;
    assert(processor->process(data) == kResultOk);
    if constexpr (std::is_same_v<Sample, float>) bus.channelBuffers32 = channels;
    else bus.channelBuffers64 = channels;
    assert(component->activateBus(kAudio, kOutput, 0, true) == kResultOk);
    events.values.clear(); check(7, silence);
    events.values = {note(true, 0, 21)}; check(7, low);
    // 処理停止でゲートを閉じる。再開しても勝手に鳴らず、再発音は位相を継承。
    assert(processor->setProcessing(false) == kResultOk);
    assert(processor->setProcessing(true) == kResultOk);
    events.values.clear(); check(7, silence);
    events.values = {note(true, 0, 22)}; check(7, low);
    assert(component->activateBus(kEvent, kInput, 0, false) == kResultOk);
    check(7, silence);
    assert(processor->setProcessing(false) == kResultOk);
    assert(component->setActive(false) == kResultOk);
    // 明示的な無効化は再開しても維持される。受信済みのNote Onも復活させない。
    assert(component->setActive(true) == kResultOk);
    assert(processor->setProcessing(true) == kResultOk);
    check(7, silence);
    assert(processor->setProcessing(false) == kResultOk);
    assert(component->setActive(false) == kResultOk);
    assert(component->activateBus(kEvent, kInput, 0, true) == kResultOk);
    assert(component->setActive(true) == kResultOk);
    assert(processor->setProcessing(true) == kResultOk);
    events.values.clear(); check(7, silence);
    events.values = {note(true, 0, 23)}; check(7, low);
    assert(processor->setProcessing(false) == kResultOk);
    assert(component->setActive(false) == kResultOk);
    configureEnvelope(processor, false);
}

// ここからも検証専用のホスト模擬。プラグインは標準VST3 ABIだけでアクセスする。
constexpr std::array<ParamID, 6> cutoffEnvelopeIds {0x43415454, 0x43484c44, 0x43444543, 0x43535553, 0x4352454c, 0x43414d54};
constexpr ParamID waveformId = 0x57415645, phaseId = 0x50484153, volumeId = 0x4d564f4c;
class Points final : public IParamValueQueue {
public:
    ParamID id = waveformId;
    std::vector<std::pair<int32, ParamValue>> values;
    int32 failedIndex = -1;
    tresult PLUGIN_API queryInterface(const TUID, void** out) override { *out = nullptr; return kNoInterface; }
    uint32 PLUGIN_API addRef() override { assert(false); return 1; }
    uint32 PLUGIN_API release() override { assert(false); return 1; }
    ParamID PLUGIN_API getParameterId() override { return id; }
    int32 PLUGIN_API getPointCount() override { return static_cast<int32>(values.size()); }
    tresult PLUGIN_API getPoint(int32 index, int32& offset, ParamValue& value) override {
        if (index == failedIndex) return kResultFalse;
        const auto& point = values.at(static_cast<size_t>(index));
        offset = point.first; value = point.second; return kResultOk;
    }
    tresult PLUGIN_API addPoint(int32, ParamValue, int32&) override { assert(false); return kNotImplemented; }
};
class Changes final : public IParameterChanges {
public:
    std::vector<IParamValueQueue*> queues;
    tresult PLUGIN_API queryInterface(const TUID, void** out) override { *out = nullptr; return kNoInterface; }
    uint32 PLUGIN_API addRef() override { assert(false); return 1; }
    uint32 PLUGIN_API release() override { assert(false); return 1; }
    int32 PLUGIN_API getParameterCount() override { return static_cast<int32>(queues.size()); }
    IParamValueQueue* PLUGIN_API getParameterData(int32 index) override { return queues.at(static_cast<size_t>(index)); }
    IParamValueQueue* PLUGIN_API addParameterData(const ParamID&, int32&) override { assert(false); return nullptr; }
};
constexpr ParamID unisonId = 0x554e4953, detuneId = 0x4454554e, polyphonyId = 0x504f4c59;
constexpr std::array<ParamID, 3> filterIds {0x46545950, 0x46435554, 0x46524553};
constexpr std::array<ParamID, 5> envelopeIds {0x4154544b, 0x484f4c44, 0x44454359, 0x53555354, 0x52454c53};
double timeNormalized(double ms) {
    return (ms <= 1.0 ? ms : 1.0 + 100.0 * std::log10(ms)) / (1.0 + 100.0 * std::log10(5000.0));
}
void configureEnvelope(IAudioProcessor* processor, bool immediate, double timeScale) {
    std::array<Points, 5> points;
    Changes changes;
    const std::array<double, 5> defaults {timeNormalized(10 * timeScale), 0.0, timeNormalized(100 * timeScale), 0.7, timeNormalized(100 * timeScale)};
    for (size_t i = 0; i < 5; ++i) {
        points[i].id = envelopeIds[i];
        points[i].values = {{0, immediate ? (i == 3 ? 1.0 : 0.0) : defaults[i]}};
        changes.queues.push_back(&points[i]);
    }
    // 発振器を直接検証する既存テストではフィルタをバイパスする。
    Points bypass; bypass.id = filterIds[0]; bypass.values = {{0, 0.0}};
    changes.queues.push_back(&bypass);
    ProcessData data; data.inputParameterChanges = &changes;
    assert(processor->process(data) == kResultOk);
}

class StateStream final : public IBStream {
public:
    std::vector<uint8> bytes;
    size_t position = 0;
    int32 chunk = 3; // 短いread/writeを繰り返しても正しい状態を構成できることを確認する。
    bool fail = false;
    tresult PLUGIN_API queryInterface(const TUID, void** out) override { *out = nullptr; return kNoInterface; }
    uint32 PLUGIN_API addRef() override { assert(false); return 1; }
    uint32 PLUGIN_API release() override { assert(false); return 1; }
    tresult PLUGIN_API read(void* out, int32 size, int32* actual) override {
        *actual = 0;
        if (fail) return kInternalError;
        const auto count = std::min(static_cast<size_t>(std::min(size, chunk)), bytes.size() - position);
        if (count == 0) return kResultFalse;
        std::memcpy(out, bytes.data() + position, count);
        position += count; *actual = static_cast<int32>(count); return kResultOk;
    }
    tresult PLUGIN_API write(void* input, int32 size, int32* actual) override {
        *actual = 0;
        if (fail) return kInternalError;
        const auto count = static_cast<size_t>(std::min(size, chunk));
        const auto* first = static_cast<const uint8*>(input);
        bytes.insert(bytes.end(), first, first + count);
        *actual = static_cast<int32>(count); return kResultOk;
    }
    tresult PLUGIN_API seek(int64, int32, int64*) override { assert(false); return kNotImplemented; }
    tresult PLUGIN_API tell(int64*) override { assert(false); return kNotImplemented; }
};

template <typename Sample>
void checkParameters(IComponent* component, IAudioProcessor* processor, IEditController* controller,
                     int32 precision, int32 mode) {
    ParameterInfo info {};
    assert(controller->getParameterInfo(0, info) == kResultOk);
    assert(info.id == waveformId && info.stepCount == 4 && info.defaultNormalizedValue == 0.0);
    assert(info.unitId == 0 && info.flags == (ParameterInfo::kCanAutomate | ParameterInfo::kIsList));
    assert(std::u16string(info.title) == u"Osc1 Waveform");
    assert(controller->getParameterInfo(32, info) == kInvalidArgument);
    const std::array<std::u16string, 5> names {u"Sine", u"Triangle", u"Square", u"Sawtooth", u"Noise"};
    for (size_t i = 0; i < names.size(); ++i) {
        const double normalized = static_cast<double>(i) / 4.0;
        String128 text {};
        assert(controller->getParamStringByValue(waveformId, normalized, text) == kResultOk);
        assert(std::u16string(text) == names[i]);
        double parsed = -1;
        assert(controller->getParamValueByString(waveformId, text, parsed) == kResultOk && parsed == normalized);
        assert(controller->normalizedParamToPlain(waveformId, normalized) == static_cast<double>(i));
        assert(controller->plainParamToNormalized(waveformId, static_cast<double>(i)) == normalized);
        assert(controller->setParamNormalized(waveformId, normalized) == kResultOk);
        assert(controller->getParamNormalized(waveformId) == normalized);
    }
    assert(controller->setParamNormalized(waveformId, std::numeric_limits<double>::quiet_NaN()) == kInvalidArgument);
    assert(controller->setParamNormalized(waveformId, -0.1) == kInvalidArgument);
    assert(controller->setParamNormalized(0, 0.0) == kInvalidArgument);
    StateStream initial;
    assert(component->getState(&initial) == kResultOk);
    assert(initial.bytes.size() == 252 && initial.bytes[4] == 12 && initial.bytes[8] == 0);
    configureEnvelope(processor, true);
    // Controllerへの表示更新だけではProcessorの値は変わらない。
    assert(controller->getParamNormalized(waveformId) == 1.0);

    ProcessSetup setup {mode, precision, 8, 56320.0}; // Note Onのtuningを4oct上げ、8サンプルで1周期にする。
    assert(processor->setupProcessing(setup) == kResultOk);
    for (double rate : {0.0, 32000.0, std::nextafter(44100.0, 0.0), std::numeric_limits<double>::infinity(), std::numeric_limits<double>::quiet_NaN()}) {
        auto rejected = setup; rejected.sampleRate = rate;
        assert(processor->setupProcessing(rejected) == kInvalidArgument);
    }
    assert(component->activateBus(kAudio, kOutput, 0, true) == kResultOk);
    Points ignored, points;
    ignored.id = 123; ignored.values = {{0, 1.0}};
    Changes changes; changes.queues = {nullptr, &ignored, &points};
    Events events; events.values = {note(true, 0, 1)};
    std::array<Sample, 10> buffer;
    Sample* channels[] = {buffer.data() + 1, buffer.data() + 1};
    AudioBusBuffers bus; bus.numChannels = 2;
    if constexpr (std::is_same_v<Sample, float>) bus.channelBuffers32 = channels;
    else bus.channelBuffers64 = channels;
    ProcessData data; data.processMode = mode; data.symbolicSampleSize = precision;
    data.inputParameterChanges = &changes;
    // 処理開始前、出力のない0サンプルのフラッシュで矩形波へ切り替える。
    points.values = {{0, 0.5}};
    assert(processor->process(data) == kResultOk);
    assert(component->setActive(true) == kResultOk);
    assert(processor->setProcessing(true) == kResultOk);
    data.numOutputs = 1; data.outputs = &bus; data.inputEvents = &events;
    auto check = [&](std::initializer_list<double> expected) {
        // レートを16倍にした分、ノート周波数も16倍にして位相の期待値を保つ。
        for (auto& event : events.values) if (event.type == Event::kNoteOnEvent) event.noteOn.tuning = 4800.0f;
        data.numSamples = static_cast<int32>(expected.size());
        buffer.fill(Sample(9));
        assert(processor->process(data) == kResultOk);
        size_t i = 1;
        bool nonzero = false;
        for (double value : expected) {
            assert(std::abs(buffer[i++] - Sample(value)) < 1e-6);
            nonzero |= value != 0.0;
        }
        assert(buffer[0] == Sample(9) && buffer[i] == Sample(9));
        assert(bus.silenceFlags == (nonzero ? 0u : 3u));
    };
    points.values.clear();
    check({0.5, 0.5}); // 位相0, 1/8。
    events.values.clear();
    const double triangle = 0.5 * std::sqrt(3.0);
    // 変更点までは矩形波を保持。最後のサンプルでノコギリ波へ切り替える。
    points.values = {{1, 0.25}, {4, 0.75}};
    check({0.5, triangle * 0.5, 0.0, -triangle * 0.5, triangle * 0.5});
    points.values.clear(); check({triangle * 0.75}); // 次ブロックにも最後の値を保持、位相7/8。
    // 同位置の変更はキュー順、ノートより先に処理。波形変更で位相はリセットしない。
    points.values = {{0, 0.25}, {0, 0.5}};
    events.values = {note(false, 0, 1), note(true, 0, 2, 81)};
    check({0.5}); // 位相0から1/4へ。
    events.values.clear();
    points.values = {{-1, 0.0}, {3, 0.0}, {0, std::numeric_limits<double>::quiet_NaN()},
                     {0, 1.1}, {0, 0.0}, {2, 0.25}, {1, 0.0}};
    points.failedIndex = 4;
    check({0.5, -0.5, -triangle}); // 無効・範囲外・逆順・取得失敗の点は無視。
    points.failedIndex = -1;
    // サイン波への単一点変更。その後の値を保持し、線形補間はしない。
    points.values = {{0, 0.0}};
    check({0.0, 0.5 * std::sqrt(2.0), 0.0, -0.5 * std::sqrt(2.0)});
    events.values = {note(false, 0, 2, 81)};
    points.values = {{0, 0.75}}; check({0.0});
    events.values.clear(); points.values.clear();
    StateStream saved;
    assert(component->getState(&saved) == kResultOk && saved.bytes[8] == 3);
    assert(controller->setComponentState(&saved) == kResultOk);
    assert(controller->getParamNormalized(waveformId) == 0.75);
    // 一体型なのでComponentの復元だけでもControllerの表示を同期する。
    // 空データは拒否し、現在の値を保持する。
    StateStream empty;
    assert(component->setState(&empty) != kResultOk);
    assert(controller->setComponentState(&empty) != kResultOk);
    assert(controller->getParamNormalized(waveformId) == 0.75);
    saved.position = 0;
    assert(component->setState(&saved) == kResultOk);
    assert(controller->getParamNormalized(waveformId) == 0.75);
    saved.position = 0;
    assert(controller->setComponentState(&saved) == kResultOk);
    StateStream own;
    assert(controller->getState(&own) == kResultOk && own.bytes.empty());
    assert(controller->setState(&own) == kResultOk);
    // 途中切れ、未対応バージョン、波形番号、magic、I/O失敗は既存の状態を壊さない。
    for (int error = 0; error < 5; ++error) {
        StateStream broken; broken.bytes = saved.bytes;
        if (error == 0) broken.bytes.resize(11);
        if (error == 1) broken.bytes[4] = 13;
        if (error == 2) broken.bytes[8] = 5;
        if (error == 3) broken.bytes[0] = 0;
        if (error == 4) broken.fail = true;
        assert(component->setState(&broken) != kResultOk);
        broken.position = 0;
        assert(controller->setComponentState(&broken) != kResultOk);
        assert(controller->getParamNormalized(waveformId) == 0.75);
        StateStream current;
        assert(component->getState(&current) == kResultOk && current.bytes == saved.bytes);
    }
    StateStream failedWrite; failedWrite.fail = true;
    assert(component->getState(&failedWrite) != kResultOk);
    assert(component->getState(nullptr) == kInvalidArgument);
    assert(component->setState(nullptr) == kInvalidArgument);
    assert(controller->setComponentState(nullptr) == kInvalidArgument);
    events.values = {note(true, 0, 3)};
    check({-triangle}); // 復元で位相を変えず、保存したノコギリ波を発音。
    assert(processor->setProcessing(false) == kResultOk);
    assert(component->setActive(false) == kResultOk);
}

template <typename Sample>
void checkEnvelope(IComponent* component, IAudioProcessor* processor, IEditController* controller, int32 precision) {
    configureEnvelope(processor, false, 1.0 / 48.0); // 48 kHzでも1サンプルごとの振幅期待値を保つ。
    ProcessSetup setup {kRealtime, precision, 16, 48000.0};
    assert(processor->setupProcessing(setup) == kResultOk);
    assert(processor->getTailSamples() == kInfiniteTail);
    assert(component->activateBus(kAudio, kOutput, 0, true) == kResultOk);
    assert(component->activateBus(kEvent, kInput, 0, true) == kResultOk);
    // 全追加パラメーターは連続値として公開し、実単位・文字列表現を往復できる。
    for (size_t i = 0; i < envelopeIds.size(); ++i) {
        ParameterInfo info;
        assert(controller->getParameterInfo(static_cast<int32>(i + 1), info) == kResultOk);
        assert(info.id == envelopeIds[i] && info.stepCount == 0 && info.flags == ParameterInfo::kCanAutomate);
        for (double value : {0.0, 1.0}) {
            const double plain = controller->normalizedParamToPlain(info.id, value);
            assert(plain == value * (i == 3 ? 1.0 : 5000.0));
            assert(controller->plainParamToNormalized(info.id, plain) == value);
            String128 text; double parsed = -1;
            assert(controller->getParamStringByValue(info.id, value, text) == kResultOk);
            assert(controller->getParamValueByString(info.id, text, parsed) == kResultOk && parsed == value);
        }
    }
    Points wave; wave.values = {{0, 0.5}};
    Changes changes; changes.queues = {&wave};
    Events events;
    std::array<Sample, 16> buffer;
    Sample* channels[] = {buffer.data(), buffer.data()};
    AudioBusBuffers bus; bus.numChannels = 2;
    if constexpr (std::is_same_v<Sample, float>) bus.channelBuffers32 = channels;
    else bus.channelBuffers64 = channels;
    ProcessData data; data.symbolicSampleSize = precision; data.inputParameterChanges = &changes;
    assert(processor->process(data) == kResultOk); // 波形の0サンプルフラッシュ。
    wave.values.clear();
    assert(component->setActive(true) == kResultOk);
    assert(processor->setProcessing(true) == kResultOk);
    data.numOutputs = 1; data.outputs = &bus; data.inputEvents = &events;
    auto check = [&](std::initializer_list<double> expected) {
        data.numSamples = static_cast<int32>(expected.size());
        assert(processor->process(data) == kResultOk);
        size_t i = 0; bool audible = false;
        for (double value : expected) {
            assert(std::abs(std::abs(buffer[i++]) - value) < 1e-6);
            audible |= value != 0.0;
        }
        assert(bus.silenceFlags == (audible ? 0u : 3u));
    };
    events.values = {note(true, 2, 1)};
    check({0, 0, 0, .05, .1, .15, .2, .25, .3, .35, .4, .45});
    // 再ノートオンは新しいベロシティが小さくても現在の実効振幅を引き継ぐ。
    events.values = {note(true, 0, 2, 81, 0, .5f), note(false, 0, 1)};
    check({.5, .475, .45, .425});
    events.values = {note(false, 0, 2, 81)}; check({.4, .396, .392});
    events.values = {note(true, 0, 3)}; check({.388, .3992});
    Points release; release.id = envelopeIds[4]; release.values = {{0, 0.0}};
    changes.queues = {&release}; events.values = {note(false, 0, 3)};
    check({0, 0}); // 同位置のrelease=0をノートオフに先立って適用。
    Points attack, decay;
    attack.id = envelopeIds[0]; attack.values = {{0, 0.0}};
    decay.id = envelopeIds[2]; decay.values = {{0, 0.0}};
    changes.queues = {&attack, &decay}; events.values = {note(true, 0, 4)};
    check({.35, .35});
    Points sustain; sustain.id = envelopeIds[3]; sustain.values = {{3, .3}};
    changes.queues = {&sustain}; events.values.clear();
    check({.3, .25, .2, .15}); // offset=-1の0.7からoffset=3の0.3へ線形補間。
    changes.queues.clear(); check({.15, .15});
    StateStream saved; assert(component->getState(&saved) == kResultOk);
    assert(saved.bytes.size() == 252 && saved.bytes[4] == 12);
    assert(controller->setComponentState(&saved) == kResultOk);
    assert(std::abs(controller->getParamNormalized(envelopeIds[3]) - .3) < 1e-12);
    // 不正な時間値はControllerにもDSPにも適用しない。
    saved.position = 0;
    const double nan = std::numeric_limits<double>::quiet_NaN();
    std::memcpy(saved.bytes.data() + 12, &nan, sizeof(nan));
    assert(component->setState(&saved) != kResultOk); saved.position = 0;
    assert(controller->setComponentState(&saved) != kResultOk);
    assert(std::abs(controller->getParamNormalized(envelopeIds[3]) - .3) < 1e-12);
    assert(processor->setProcessing(false) == kResultOk);
    assert(component->setActive(false) == kResultOk);
}

template <typename Sample>
void checkUnison(IComponent* component, IAudioProcessor* processor, IEditController* controller,
                 int32 precision) {
    configureEnvelope(processor, true);
    for (int index : {6, 7}) {
        ParameterInfo info;
        assert(controller->getParameterInfo(index, info) == kResultOk);
        assert(info.id == (index == 6 ? unisonId : detuneId));
        assert(info.stepCount == (index == 6 ? 6 : 0) && info.flags == ParameterInfo::kCanAutomate);
        assert(info.defaultNormalizedValue == 0.0);
        assert(std::u16string(info.units) == (index == 6 ? u"" : u"cent"));
        for (double plain : (index == 6 ? std::array<double, 3>{1, 4, 7} : std::array<double, 3>{0, 37, 100})) {
            const double value = controller->plainParamToNormalized(info.id, plain);
            assert(std::abs(controller->normalizedParamToPlain(info.id, value) - plain) < 1e-12);
            String128 text; double parsed = -1;
            assert(controller->getParamStringByValue(info.id, value, text) == kResultOk);
            assert(controller->getParamValueByString(info.id, text, parsed) == kResultOk);
            assert(std::abs(parsed - value) < 1e-12);
        }
    }
    ProcessSetup setup {kRealtime, precision, 16, 48000.0};
    assert(processor->setupProcessing(setup) == kResultOk);
    assert(component->activateBus(kAudio, kOutput, 0, true) == kResultOk);
    assert(component->setActive(true) == kResultOk);
    assert(processor->setProcessing(true) == kResultOk);
    std::array<Sample, 16> buffer;
    Sample* channels[] = {buffer.data(), buffer.data()};
    AudioBusBuffers bus; bus.numChannels = 2;
    if constexpr (std::is_same_v<Sample, float>) bus.channelBuffers32 = channels;
    else bus.channelBuffers64 = channels;
    Events events; events.values = {note(true, 0, 1)};
    Points voices, detune; voices.id = unisonId; detune.id = detuneId;
    Changes changes; changes.queues = {&voices, &detune};
    ProcessData data; data.symbolicSampleSize = precision; data.numSamples = 16;
    data.numOutputs = 1; data.outputs = &bus; data.inputEvents = &events;
    data.inputParameterChanges = &changes;
    std::array<double, 7> phases {};
    int previousCount = 1;
    double previousDetune = 0;
    // ボイス数の段差とDetuneのランプを、音声サンプルの独立した参照計算と比較。
    for (int n = 1; n <= 7; ++n) {
        const double target = n % 2 ? 100.0 : 25.0;
        voices.values = {{3, (n - 1) / 6.0}};
        detune.values = {{15, target / 100.0}};
        assert(processor->process(data) == kResultOk);
        for (int sample = 0; sample < 16; ++sample) {
            const int count = sample < 3 ? previousCount : n;
            const double cents = previousDetune + (target - previousDetune) * (sample + 1) / 16.0;
            double expected = 0;
            for (int k = 0; k < count; ++k) {
                expected += std::sin(2 * std::acos(-1.0) * phases[k]);
                const double spread = count == 1 ? 0 : cents * (-1 + 2.0 * k / (count - 1));
                phases[k] = std::fmod(phases[k] + 440.0 * std::pow(2.0, spread / 1200.0) / 48000.0, 1.0);
            }
            expected *= .5 * std::sqrt(2.0 / count);
            assert(std::abs(buffer[sample] - expected) < 1e-6);
        }
        events.values.clear(); previousCount = n; previousDetune = target;
    }
    StateStream saved; assert(component->getState(&saved) == kResultOk);
    assert(saved.bytes.size() == 252 && saved.bytes[4] == 12);
    assert(controller->setComponentState(&saved) == kResultOk);
    assert(controller->getParamNormalized(unisonId) == 1.0);
    assert(controller->getParamNormalized(detuneId) == 1.0);
    // Detune=0は過去の位相差によらず先頭ボイスだけで出力する。
    voices.values.clear(); detune.values = {{0, 0.0}};
    assert(processor->process(data) == kResultOk);
    for (int sample = 0; sample < 16; ++sample) {
        const double expected = .5 * std::sqrt(2.0) * std::sin(2 * std::acos(-1.0) * phases[0]);
        assert(std::abs(buffer[sample] - expected) < 1e-6);
        phases[0] = std::fmod(phases[0] + 440.0 / 48000.0, 1.0);
    }
    // 現行形式をDSPへ復元し、全パラメーターを保持する。
    saved.position = 0;
    assert(component->setState(&saved) == kResultOk);
    StateStream roundtrip; assert(component->getState(&roundtrip) == kResultOk);
    assert(roundtrip.bytes == saved.bytes);
    assert(processor->setProcessing(false) == kResultOk);
    assert(component->setActive(false) == kResultOk);
}

template <typename Sample>
void checkPolyphony(IComponent* component, IAudioProcessor* processor, IEditController* controller,
                    int32 precision, int32 mode) {
    ParameterInfo info;
    assert(controller->getParameterInfo(8, info) == kResultOk);
    assert(info.id == polyphonyId && info.stepCount == 31 && info.flags == ParameterInfo::kCanAutomate);
    assert(std::u16string(info.title) == u"Polyphony" && info.defaultNormalizedValue == 0.0);
    for (int count = 1; count <= 32; ++count) {
        const double value = controller->plainParamToNormalized(polyphonyId, count);
        assert(controller->normalizedParamToPlain(polyphonyId, value) == count);
        String128 text; double parsed = -1;
        assert(controller->getParamStringByValue(polyphonyId, value, text) == kResultOk);
        assert(controller->getParamValueByString(polyphonyId, text, parsed) == kResultOk && parsed == value);
    }
    configureEnvelope(processor, true);
    ProcessSetup setup {mode, precision, 8, 48000.0};
    assert(processor->setupProcessing(setup) == kResultOk);
    assert(component->activateBus(kAudio, kOutput, 0, true) == kResultOk);
    Points wave, poly; wave.values = {{0, 0.5}};
    poly.id = polyphonyId; poly.values = {{0, 1.0 / 31.0}};
    Changes changes; changes.queues = {&wave, &poly};
    ProcessData data; data.symbolicSampleSize = precision; data.inputParameterChanges = &changes;
    assert(processor->process(data) == kResultOk); // 停止中の0サンプルで2枠へ変更。
    changes.queues = {&poly}; poly.values.clear();
    std::array<Sample, 8> buffer;
    Sample* channels[] = {buffer.data(), buffer.data()};
    AudioBusBuffers bus; bus.numChannels = 2;
    if constexpr (std::is_same_v<Sample, float>) bus.channelBuffers32 = channels;
    else bus.channelBuffers64 = channels;
    Events events;
    data.numOutputs = 1; data.outputs = &bus; data.inputEvents = &events;
    assert(component->setActive(true) == kResultOk);
    assert(processor->setProcessing(true) == kResultOk);
    auto check = [&](std::initializer_list<double> expected) {
        data.numSamples = static_cast<int32>(expected.size());
        assert(processor->process(data) == kResultOk);
        size_t i = 0; bool audible = false;
        for (double value : expected) {
            assert(std::abs(buffer[i++] - value) < 1e-6);
            audible |= value != 0.0;
        }
        assert(bus.silenceFlags == (audible ? 0u : 3u));
    };
    // 低い音程で矩形波の正側を維持し、サンプルごとの加算値を検証。
    events.values = {note(true, 1, 1, 12), note(true, 3, 2, 16)};
    check({0, .5, .5, 1, 1, 1});
    events.values = {note(true, 2, 3, 19, 0, .5f), note(false, 2, 1, 12), note(false, 4, 2, 16)};
    check({1, 1, .75, .75, .25, .25});
    events.values = {note(false, 1, 3, 19)}; check({.25, 0, 0});
    // 32枠を同時発音し、上限で循環しても旧ノートのNote Offは新しい音に影響しない。
    poly.values = {{0, 1.0}}; events.values.clear();
    for (int id = 0; id < 32; ++id) events.values.push_back(note(true, 0, 100 + id, 12));
    check({16, 16});
    poly.values.clear();
    events.values = {note(true, 0, 200, 12, 0, .5f), note(false, 0, 100, 12)};
    check({15.75, 15.75});
    StateStream saved; assert(component->getState(&saved) == kResultOk);
    assert(saved.bytes.size() == 252 && saved.bytes[4] == 12);
    assert(controller->setComponentState(&saved) == kResultOk);
    assert(controller->getParamNormalized(polyphonyId) == 1.0);
    // 次の割り当て位置は2。減少で範囲外の音を止め、増加しても復活させない。
    events.values.clear(); poly.values = {{1, 1.0 / 31.0}};
    check({15.75, .75, .75});
    poly.values = {{1, 1.0}}; check({.75, .75, .75});
    assert(processor->setProcessing(false) == kResultOk);
    assert(component->setActive(false) == kResultOk);
}

// 検証専用: フィルタの回路式をホスト側で計算し、配送された設定・時刻・ノートを照合する。
struct FilterReference {
    double state[2][2] {};
    double process(double input, int type, double hz, double q, double rate) {
        if (type == 0) { *this = {}; return input; }
        const double g = std::tan(std::acos(-1.0) * std::min(.49, std::clamp(hz, 20.0, 20000.0) / rate));
        const int stages = type % 2 == 0 ? 2 : 1;
        for (int stage = 0; stage < stages; ++stage) {
            auto& s = state[stage];
            const double high = (input - (g + 1 / q) * s[0] - s[1]) / (1 + g * (g + 1 / q));
            const double band = g * high + s[0], low = g * band + s[1];
            s[0] = 2 * band - s[0]; s[1] = 2 * low - s[1];
            input = type <= 2 ? low : (type <= 4 ? high : band);
        }
        return input;
    }
};

template <typename Sample>
void checkFilter(IComponent* component, IAudioProcessor* processor, IEditController* controller, int32 precision) {
    for (int i = 0; i < 3; ++i) {
        ParameterInfo info;
        assert(controller->getParameterInfo(9 + i, info) == kResultOk && info.id == filterIds[i]);
        assert(info.stepCount == (i == 0 ? 6 : 0));
        const double initial = controller->normalizedParamToPlain(info.id, info.defaultNormalizedValue);
        assert(std::abs(initial - (i == 0 ? 1.0 : (i == 1 ? 4800.0 : 1.0))) < 1e-12);
    }
    for (int tick = 0; tick <= 1200; ++tick) {
        const double cents = -2400.0 + 10 * tick;
        const double normalized = controller->plainParamToNormalized(filterIds[1], cents);
        assert(std::abs(controller->normalizedParamToPlain(filterIds[1], normalized) - cents) < 2e-12);
    }
    const std::array<std::u16string, 7> names {u"None", u"LowPass1", u"LowPass2", u"HighPass1", u"HighPass2", u"BandPass1", u"BandPass2"};
    configureEnvelope(processor, true);
    ProcessSetup setup {kRealtime, precision, 16, 48000.0};
    assert(processor->setupProcessing(setup) == kResultOk);
    assert(component->activateBus(kAudio, kOutput, 0, true) == kResultOk);
    assert(component->setActive(true) == kResultOk);
    std::array<Sample, 16> buffer {};
    Sample* channels[] = {buffer.data(), buffer.data()};
    AudioBusBuffers bus; bus.numChannels = 2;
    if constexpr (std::is_same_v<Sample, float>) bus.channelBuffers32 = channels;
    else bus.channelBuffers64 = channels;
    Points type, cutoff, resonance, wave, poly;
    type.id = filterIds[0]; cutoff.id = filterIds[1]; resonance.id = filterIds[2]; poly.id = polyphonyId;
    Changes changes; changes.queues = {&type, &cutoff, &resonance, &wave, &poly};
    Events events;
    ProcessData data; data.symbolicSampleSize = precision; data.inputParameterChanges = &changes;
    for (int t = 0; t < 7; ++t) {
        String128 text; double parsed = -1;
        assert(controller->getParamStringByValue(filterIds[0], t / 6.0, text) == kResultOk && std::u16string(text) == names[t]);
        assert(controller->getParamValueByString(filterIds[0], text, parsed) == kResultOk && parsed == t / 6.0);
        const double q0 = 1 / std::sqrt(2.0), q1 = 4.0;
        type.values = {{0, t / 6.0}}; cutoff.values = {{0, .3}};
        resonance.values = {{0, (q0 - .5) / 9.5}}; wave.values = {{0, 0.5}}; poly.values = {{0, 1.0 / 31.0}};
        data.numSamples = 0; data.numOutputs = 0; data.inputEvents = nullptr;
        assert(processor->process(data) == kResultOk); // 停止中のフラッシュ。
        assert(processor->setProcessing(true) == kResultOk);
        type.values.clear(); wave.values.clear(); poly.values.clear();
        cutoff.values = {{3, 0.0}, {7, 1.0}}; resonance.values = {{7, (q1 - .5) / 9.5}};
        events.values = {note(true, 0, 1, 12), note(true, 2, 2, 24)};
        data.numSamples = 12; data.numOutputs = 1; data.outputs = &bus; data.inputEvents = &events;
        assert(processor->process(data) == kResultOk);
        FilterReference refs[2];
        for (int n = 0; n < 12; ++n) {
            const double cents = n < 3 ? 1200 : (n < 7 ? -2400 : 9600);
            const double q = n < 8 ? q0 + (q1 - q0) * (n + 1) / 8.0 : q1;
            double expected = 0;
            for (int v = 0; v < 2; ++v) {
                // Note On前のVoiceは無音。発音後はノートごとのカットオフで独立処理。
                const double hz = 440 * std::pow(2.0, (12 + 12 * v - 69) / 12.0) * std::pow(2.0, cents / 1200.0);
                expected += refs[v].process(n >= v * 2 ? .5 : 0, t, hz, q, 48000);
            }
            assert(std::abs(buffer[n] - expected) < 1e-5);
        }
        assert(bus.silenceFlags == 0);
        StateStream saved; assert(component->getState(&saved) == kResultOk && saved.bytes.size() == 252);
        assert(controller->setComponentState(&saved) == kResultOk);
        assert(controller->getParamNormalized(filterIds[0]) == t / 6.0);
        assert(controller->getParamNormalized(filterIds[1]) == 1.0);
        assert(std::abs(controller->normalizedParamToPlain(filterIds[2], controller->getParamNormalized(filterIds[2])) - q1) < 1e-12);
        assert(processor->setProcessing(false) == kResultOk);
    }
    assert(component->setActive(false) == kResultOk);
}

// 3発振器のVST3キューを通し、Tuneランプと−∞を実際の出力で検証する。
template <typename Sample>
void checkThreeOscillators(IComponent* component, IAudioProcessor* processor, IEditController* controller, int32 precision) {
    configureEnvelope(processor, true);
    ProcessSetup setup {kRealtime, precision, 33, 48000.0};
    assert(processor->setupProcessing(setup) == kResultOk);
    assert(component->activateBus(kAudio, kOutput, 0, true) == kResultOk);
    std::array<Sample, 33> left {}, right {};
    Sample* channels[] = {left.data(), right.data()};
    AudioBusBuffers bus; bus.numChannels = 2;
    if constexpr (std::is_same_v<Sample, float>) bus.channelBuffers32 = channels;
    else bus.channelBuffers64 = channels;
    std::array<Points, 8> points;
    constexpr std::array<int, 8> indices {22, 23, 24, 25, 26, 27, 28, 12};
    const std::array<double, 8> initial {-6, 0, -1200, -3, 0, 1200, 0, 1};
    Changes changes;
    for (size_t i = 0; i < points.size(); ++i) {
        ParameterInfo info;
        assert(controller->getParameterInfo(indices[i], info) == kResultOk);
        points[i].id = info.id;
        points[i].values = {{0, controller->plainParamToNormalized(info.id, initial[i])}};
        changes.queues.push_back(&points[i]);
    }
    for (const int index : {13, 22, 25, 28}) {
        ParameterInfo info; assert(controller->getParameterInfo(index, info) == kResultOk);
        assert(controller->normalizedParamToPlain(info.id, 0) == -std::numeric_limits<double>::infinity());
        String128 text; ParamValue value = -1;
        assert(controller->getParamStringByValue(info.id, 0, text) == kResultOk && std::u16string(text) == u"−∞");
        assert(controller->getParamValueByString(info.id, text, value) == kResultOk && value == 0);
    }
    ProcessData data; data.symbolicSampleSize = precision; data.inputParameterChanges = &changes;
    assert(processor->process(data) == kResultOk);
    assert(component->setActive(true) == kResultOk);
    assert(processor->setProcessing(true) == kResultOk);
    for (auto& point : points) point.values.clear();
    points[2].values = {{16, controller->plainParamToNormalized(points[2].id, 1200)}};
    Events events; events.values = {note(true, 0, 1), note(true, 23, 2), note(false, 31, 2)};
    data.numSamples = 33; data.numOutputs = 1; data.outputs = &bus; data.inputEvents = &events;
    assert(processor->process(data) == kResultOk);
    std::array<double, 3> phase {};
    for (int n = 0; n < 33; ++n) {
        if (n == 23) phase.fill(0); // 共通Phase=Zeroを全発振器へ適用。
        const double tune = n < 17 ? -1200 + 2400 * (n + 1) / 17.0 : 1200;
        const std::array<double, 3> hz {440, 440 * std::pow(2., tune / 1200), 880};
        double expected = 0;
        for (int i = 0; i < 3; ++i) {
            if (n < 31) expected += .5 * std::sqrt(2.) * std::sin(2 * std::acos(-1.) * phase[i]) * std::pow(10., (-6 + 3 * i) / 20.);
            phase[i] = std::fmod(phase[i] + hz[i] / 48000., 1.);
        }
        assert(std::abs(left[n] - expected) < 8e-7 && left[n] == right[n]);
    }
    // 発音中の1発振器だけを有効にし、各VolとMasterの完全消音を確認する。
    auto flush = [&](int index, double plain) {
        ParameterInfo info; assert(controller->getParameterInfo(index, info) == kResultOk);
        points[0].id = info.id;
        points[0].values = {{0, controller->plainParamToNormalized(info.id, plain)}};
        changes.queues = {&points[0]};
        data.numSamples = 0; data.numOutputs = 0; data.inputEvents = nullptr;
        assert(processor->process(data) == kResultOk);
        points[0].values.clear();
    };
    const double mute = -std::numeric_limits<double>::infinity();
    for (const int index : {22, 25, 28}) flush(index, mute);
    events.values = {note(true, 0, 3)};
    auto render = [&] {
        data.numSamples = 33; data.numOutputs = 1;
        assert(processor->process(data) == kResultOk);
    };
    data.inputEvents = &events; render(); data.inputEvents = nullptr;
    for (const int index : {22, 25, 28}) {
        flush(index, 0); render();
        assert(std::any_of(left.begin(), left.end(), [](Sample x) { return x != 0; }));
        flush(13, mute); render();
        for (auto sample : left) assert(sample == 0);
        flush(13, 0); flush(index, mute); render();
        for (auto sample : left) assert(sample == 0);
    }
    assert(processor->setProcessing(false) == kResultOk);
    assert(component->setActive(false) == kResultOk);
}

template <typename Sample>
void checkFm(IComponent* component, IAudioProcessor* processor, IEditController* controller, int32 precision) {
    constexpr ParamID fm2Id = 0x4f32464d, fm3Id = 0x4f33464d;
    for (int index = 29; index <= 30; ++index) {
        ParameterInfo info; assert(controller->getParameterInfo(index, info) == kResultOk);
        assert(info.id == (index == 29 ? fm2Id : fm3Id));
        assert(info.stepCount == 0 && info.defaultNormalizedValue == 0 && info.flags == ParameterInfo::kCanAutomate);
        assert(controller->plainParamToNormalized(info.id, 100) == 1);
        String128 text; ParamValue value = -1;
        assert(controller->getParamStringByValue(info.id, .375, text) == kResultOk);
        assert(controller->getParamValueByString(info.id, text, value) == kResultOk && value == .375);
    }
    configureEnvelope(processor, true);
    ProcessSetup setup {kRealtime, precision, 33, 48000.0};
    assert(processor->setupProcessing(setup) == kResultOk);
    assert(component->activateBus(kAudio, kOutput, 0, true) == kResultOk);
    // Osc2/3は既定の−∞のまま。Pan右端でも変調信号は変わらない。
    std::array<Points, 7> points;
    const std::array<int, 7> indices {0, 12, 21, 24, 27, 29, 30};
    const std::array<double, 7> plain {3, 1, 100, 1200, -1200, 25, 50};
    Changes changes;
    for (size_t i = 0; i < points.size(); ++i) {
        ParameterInfo info; assert(controller->getParameterInfo(indices[i], info) == kResultOk);
        points[i].id = info.id;
        points[i].values = {{0, controller->plainParamToNormalized(info.id, plain[i])}};
        changes.queues.push_back(&points[i]);
    }
    ProcessData data; data.symbolicSampleSize = precision; data.inputParameterChanges = &changes;
    assert(processor->process(data) == kResultOk);
    assert(component->setActive(true) == kResultOk);
    assert(processor->setProcessing(true) == kResultOk);
    for (auto& p : points) p.values.clear();
    points[5].values = {{15, 1.0}};
    points[6].values = {{8, .5}, {16, 1.0}};
    Events events; events.values = {note(true, 0, 1), note(true, 23, 2), note(false, 31, 2)};
    std::array<Sample, 33> left {}, right {};
    Sample* channels[] = {left.data(), right.data()};
    AudioBusBuffers bus; bus.numChannels = 2;
    if constexpr (std::is_same_v<Sample, float>) bus.channelBuffers32 = channels;
    else bus.channelBuffers64 = channels;
    data.numSamples = 33; data.numOutputs = 1; data.outputs = &bus; data.inputEvents = &events;
    assert(processor->process(data) == kResultOk);
    std::array<double, 3> phase {};
    for (int n = 0; n < 33; ++n) {
        if (n == 23) phase.fill(0);
        const double expected = n < 31 ? .5 * std::sqrt(6.) * (2 * phase[0] - 1) : 0;
        assert(left[n] == 0 && std::abs(right[n] - expected) < 3e-6);
        const double fm2 = n < 16 ? .25 + .75 * (n + 1) / 16. : 1;
        const double fm3 = n <= 8 ? .5 : n < 16 ? .5 + .5 * (n - 8) / 8. : 1;
        const double m2 = std::sin(2 * std::acos(-1.) * phase[1]);
        const double m3 = std::sin(2 * std::acos(-1.) * phase[2]);
        phase[0] = std::fmod(phase[0] + 440 * (1 + fm2 * m2) / 48000., 1.);
        phase[1] = std::fmod(phase[1] + 880 * (1 + fm3 * m3) / 48000., 1.);
        phase[2] = std::fmod(phase[2] + 220 / 48000., 1.);
    }
    // 無効化を0サンプルで受理し、次のNote Onは無変調になる。
    points[5].values = {{0, 0}}; points[6].values = {{0, 0}};
    data.numSamples = 0; data.numOutputs = 0; data.inputEvents = nullptr;
    assert(processor->process(data) == kResultOk);
    points[5].values.clear(); points[6].values.clear();
    events.values = {note(true, 0, 3)};
    data.numSamples = 33; data.numOutputs = 1; data.inputEvents = &events;
    assert(processor->process(data) == kResultOk);
    for (int n = 0; n < 33; ++n) {
        const double expected = .5 * std::sqrt(6.) * (2 * n * 440 / 48000. - 1);
        assert(left[n] == 0 && std::abs(right[n] - expected) < 3e-7);
    }
    assert(processor->setProcessing(false) == kResultOk);
    assert(component->setActive(false) == kResultOk);
}

template <typename Sample>
void checkPhase(IComponent* component, IAudioProcessor* processor, IEditController* controller, int32 precision) {
    ParameterInfo info;
    assert(controller->getParameterInfo(12, info) == kResultOk && info.id == phaseId);
    assert(info.stepCount == 2 && info.defaultNormalizedValue == 0.0);
    assert(info.flags == (ParameterInfo::kCanAutomate | ParameterInfo::kIsList));
    const std::array<std::u16string, 3> names {u"Keep", u"Zero", u"Random"};
    for (int i = 0; i < 3; ++i) {
        String128 text; double parsed = -1;
        assert(controller->getParamStringByValue(phaseId, i / 2.0, text) == kResultOk);
        assert(std::u16string(text) == names[i]);
        assert(controller->getParamValueByString(phaseId, text, parsed) == kResultOk && parsed == i / 2.0);
    }
    configureEnvelope(processor, true);
    ProcessSetup setup {kRealtime, precision, 8, 56320.0};
    assert(processor->setupProcessing(setup) == kResultOk);
    assert(component->activateBus(kAudio, kOutput, 0, true) == kResultOk);
    Points phase, waveform;
    phase.id = phaseId; waveform.values = {{0, 0.75}};
    Changes changes; changes.queues = {&phase, &waveform};
    ProcessData data; data.symbolicSampleSize = precision; data.inputParameterChanges = &changes;
    assert(processor->process(data) == kResultOk);
    waveform.values.clear();
    assert(component->setActive(true) == kResultOk);
    assert(processor->setProcessing(true) == kResultOk);
    std::array<Sample, 8> buffer {};
    Sample* channels[] = {buffer.data(), buffer.data()};
    AudioBusBuffers bus; bus.numChannels = 2;
    if constexpr (std::is_same_v<Sample, float>) bus.channelBuffers32 = channels;
    else bus.channelBuffers64 = channels;
    Events events;
    data.numOutputs = 1; data.outputs = &bus; data.inputEvents = &events;
    const double peak = .5 * std::sqrt(3.0);
    auto check = [&](std::initializer_list<double> expected) {
        // レートを16倍にした分、ノート周波数も16倍にして位相の期待値を保つ。
        for (auto& event : events.values) if (event.type == Event::kNoteOnEvent) event.noteOn.tuning = 4800.0f;
        data.numSamples = static_cast<int32>(expected.size());
        assert(processor->process(data) == kResultOk);
        size_t i = 0;
        for (double value : expected) assert(std::abs(buffer[i++] - value * peak) < 1e-6);
    };
    events.values = {note(true, 0, 1)};
    check({-1, -.75, -.5});
    // 同位置ではモード変更をNote Onより先に適用。指定サンプルで位相0へ戻す。
    phase.values = {{1, .5}}; events.values = {note(true, 1, 2)};
    check({-.25, -1, -.75});
    phase.values = {{0, 0}}; events.values.clear(); check({-.5});
    phase.values = {{0, .5}}; check({-.25}); // モード選択だけでは位相を変えない。
    phase.values.clear(); events.values = {note(true, 0, 3)}; check({-1});
    phase.values = {{0, 1}}; events.values.clear(); check({-.75});
    phase.values.clear(); events.values = {note(true, 0, 4)};
    assert(processor->process(data) == kResultOk);
    assert(std::isfinite(buffer[0]) && std::abs(buffer[0]) <= peak);
    assert(processor->setProcessing(false) == kResultOk);
    assert(component->setActive(false) == kResultOk);
}

template <typename Sample>
void checkVolume(IComponent* component, IAudioProcessor* processor, IEditController* controller, int32 precision) {
    ParameterInfo info;
    assert(controller->getParameterInfo(13, info) == kResultOk && info.id == volumeId);
    assert(std::u16string(info.title) == u"Volume" && std::u16string(info.units) == u"dB");
    assert(info.stepCount == 0 && info.defaultNormalizedValue == 481.0 / 601.0);
    for (double db : {-48.0, -12.3, 0.0, 12.0}) {
        const double normalized = ((db + 48) * 10 + 1) / 601;
        String128 text; double parsed = -1;
        assert(controller->getParamStringByValue(volumeId, normalized, text) == kResultOk);
        assert(controller->getParamValueByString(volumeId, text, parsed) == kResultOk);
        assert(std::abs(parsed - normalized) < 1e-12);
    }
    configureEnvelope(processor, true);
    Points volume, waveform, polyphony;
    volume.id = volumeId; polyphony.id = polyphonyId;
    waveform.values = {{0, 0.5}}; polyphony.values = {{0, 1.0 / 31.0}};
    Changes changes; changes.queues = {&volume, &waveform, &polyphony};
    ProcessData data; data.symbolicSampleSize = precision; data.inputParameterChanges = &changes;
    assert(processor->process(data) == kResultOk);
    waveform.values.clear(); polyphony.values.clear();
    ProcessSetup setup {kRealtime, precision, 8, 48000.0};
    assert(processor->setupProcessing(setup) == kResultOk);
    assert(component->activateBus(kAudio, kOutput, 0, true) == kResultOk);
    assert(component->setActive(true) == kResultOk && processor->setProcessing(true) == kResultOk);
    std::array<Sample, 8> buffer {};
    Sample* channels[] = {buffer.data(), buffer.data()};
    AudioBusBuffers bus; bus.numChannels = 2;
    if constexpr (std::is_same_v<Sample, float>) bus.channelBuffers32 = channels;
    else bus.channelBuffers64 = channels;
    Events events; events.values = {note(true, 0, 1), note(true, 0, 2)};
    data.numOutputs = 1; data.outputs = &bus; data.inputEvents = &events;
    auto check = [&](std::initializer_list<double> decibels) {
        data.numSamples = static_cast<int32>(decibels.size());
        assert(processor->process(data) == kResultOk);
        size_t i = 0;
        // 振幅0.5の矩形波2ボイスを加算した後に、dBの振幅倍率を掛ける。
        for (double db : decibels) assert(std::abs(buffer[i++] - std::pow(10.0, db / 20.0)) < 1e-6);
    };
    check({0}); events.values.clear();
    volume.values = {{0, 1.0/601}, {3, 241.0/601}}; check({-48, -40, -32, -24});
    volume.values.clear(); check({-24, -24});
    volume.values = {{0, 1}, {1, 1}, {3, 481.0/601}}; check({12, 12, 6, 0});
    volume.values = {{0, ((48 - 12.3) * 10 + 1) / 601}}; check({-12.3});
    assert(processor->setProcessing(false) == kResultOk && component->setActive(false) == kResultOk);
    // 停止中のゼロサンプル変更も保存・復元へ反映する。
    data.numSamples = 0; volume.values = {{0, 0}};
    assert(processor->process(data) == kResultOk);
    StateStream saved; assert(component->getState(&saved) == kResultOk);
    assert(controller->setComponentState(&saved) == kResultOk);
    assert(controller->getParamNormalized(volumeId) == 0);
}

template <typename Sample>
void checkPanning(IComponent* component, IAudioProcessor* processor, IEditController* controller, int32 precision) {
    constexpr ParamID widthId = 0x57494454, panId = 0x4d50414e;
    for (int i : {20, 21}) {
        ParameterInfo info;
        assert(controller->getParameterInfo(i, info) == kResultOk);
        assert(info.id == (i == 20 ? widthId : panId) && info.stepCount == 0);
        assert(std::u16string(info.title) == (i == 20 ? u"Width" : u"Pan"));
        assert(std::u16string(info.units) == u"%" && info.defaultNormalizedValue == (i == 20 ? 0 : .5));
    }
    configureEnvelope(processor, true);
    Points waveform, unison, detune, width, pan, filter;
    waveform.values = {{0, 0.5}};
    unison.id = unisonId; unison.values = {{0, 1.0 / 6.0}};
    detune.id = detuneId; detune.values = {{0, .25}};
    width.id = widthId; pan.id = panId;
    filter.id = filterIds[0]; filter.values = {{0, 0}};
    Changes changes; changes.queues = {&waveform, &unison, &detune, &width, &pan, &filter};
    ProcessData data; data.symbolicSampleSize = precision; data.inputParameterChanges = &changes;
    assert(processor->process(data) == kResultOk); // 0サンプルで新設定を受理する。
    waveform.values.clear(); unison.values.clear(); detune.values.clear(); filter.values.clear();
    ProcessSetup setup {kRealtime, precision, 8, 48000.0};
    assert(processor->setupProcessing(setup) == kResultOk);
    assert(component->activateBus(kAudio, kOutput, 0, true) == kResultOk);
    assert(component->setActive(true) == kResultOk && processor->setProcessing(true) == kResultOk);
    std::array<Sample, 8> left {}, right {};
    Sample* channels[] = {left.data(), right.data()};
    AudioBusBuffers bus; bus.numChannels = 2;
    if constexpr (std::is_same_v<Sample, float>) bus.channelBuffers32 = channels;
    else bus.channelBuffers64 = channels;
    Events events; events.values = {note(true, 0, 1)};
    data.numSamples = 8; data.numOutputs = 1; data.outputs = &bus; data.inputEvents = &events;
    width.values = {{3, 1}}; pan.values = {{7, 1}};
    assert(processor->process(data) == kResultOk);
    for (int i = 0; i < 8; ++i) {
        const double w = std::min(1.0, (i + 1) / 4.0), p = (i + 1) / 8.0;
        double l = 0, r = 0;
        for (double side : {-1.0, 1.0}) {
            const double angle = (1 + p + (1 - p) * w * side) * std::acos(-1.0) / 4;
            l += .5 * std::cos(angle); r += .5 * std::sin(angle);
        }
        assert(std::abs(left[i] - l) < 1e-6 && std::abs(right[i] - r) < 1e-6);
    }
    assert(bus.silenceFlags == 0);
    width.values.clear(); pan.values.clear(); events.values.clear();
    assert(processor->process(data) == kResultOk); // 最終値を次のブロックでも保持。
    assert(bus.silenceFlags == 1);
    for (int i = 0; i < 8; ++i) assert(left[i] == 0 && std::abs(right[i] - 1.0) < 1e-6);
    pan.values = {{0, 0}};
    assert(processor->process(data) == kResultOk);
    assert(bus.silenceFlags == 2);
    for (int i = 0; i < 8; ++i) assert(right[i] == 0 && std::abs(left[i] - 1.0) < 1e-6);
    assert(processor->setProcessing(false) == kResultOk && component->setActive(false) == kResultOk);
}

template <typename Sample>
void checkCutoffEnvelope(IComponent* component, IAudioProcessor* processor, IEditController* controller, int32 precision) {
    const std::array<double, 6> initial {10, 0, 100, .7, 100, 0};
    const std::array<double, 6> settings {4.0 / 48, 2.0 / 48, 4.0 / 48, .25, 4.0 / 48, 1200};
    std::array<Points, 9> points;
    Changes changes;
    for (size_t i = 0; i < cutoffEnvelopeIds.size(); ++i) {
        ParameterInfo info;
        assert(controller->getParameterInfo(static_cast<int32>(14 + i), info) == kResultOk);
        assert(info.id == cutoffEnvelopeIds[i] && info.stepCount == 0 && info.flags == ParameterInfo::kCanAutomate);
        assert(std::abs(controller->normalizedParamToPlain(info.id, info.defaultNormalizedValue) - initial[i]) < 1e-9);
        points[i].id = info.id;
        points[i].values = {{0, controller->plainParamToNormalized(info.id, settings[i])}};
        changes.queues.push_back(&points[i]);
    }
    configureEnvelope(processor, true);
    points[6].id = waveformId; points[6].values = {{0, 0.5}};
    points[7].id = filterIds[0]; points[7].values = {{0, 2.0 / 6.0}};
    points[8].id = filterIds[1]; points[8].values = {{0, .1}}; // -1200 cent
    for (int i = 6; i < 9; ++i) changes.queues.push_back(&points[i]);
    ProcessData data; data.symbolicSampleSize = precision; data.inputParameterChanges = &changes;
    assert(processor->process(data) == kResultOk);
    ProcessSetup setup {kRealtime, precision, 32, 48000.0};
    assert(processor->setupProcessing(setup) == kResultOk);
    assert(component->activateBus(kAudio, kOutput, 0, true) == kResultOk);
    assert(component->setActive(true) == kResultOk && processor->setProcessing(true) == kResultOk);
    // Amountの保持区間・正負をまたぐランプと、ノートの再割り当てを同一ブロックで検証。
    changes.queues = {&points[5]};
    points[5].values = {{0, .625}, {3, .625}, {7, .375}, {8, .375}, {23, 1}};
    Events events; events.values = {note(true, 0, 1), note(false, 8, 1), note(true, 10, 2), note(false, 19, 2)};
    data.numSamples = 24; data.inputEvents = &events;
    std::array<Sample, 24> buffer {};
    Sample* channels[] = {buffer.data(), buffer.data()};
    AudioBusBuffers bus; bus.numChannels = 2;
    if constexpr (std::is_same_v<Sample, float>) bus.channelBuffers32 = channels;
    else bus.channelBuffers64 = channels;
    data.numOutputs = 1; data.outputs = &bus;
    assert(processor->process(data) == kResultOk);
    const std::array<double, 24> levels {0, .25, .5, .75, 1, 1, 1, .8125, .625, .46875,
        .3125, .484375, .65625, .828125, 1, 1, 1, .8125, .625, .4375, .328125, .21875, .109375, 0};
    FilterReference reference;
    double phase = 0;
    for (int n = 0; n < 24; ++n) {
        const bool gate = n < 8 || (n >= 10 && n < 19);
        const double source = gate ? (phase < .5 ? .5 : -.5) : 0;
        if (gate) phase = std::fmod(phase + 440.0 / setup.sampleRate, 1.0);
        const double amount = n <= 3 ? 1200 : (n <= 7 ? 1200 - (n - 3) * 600 : -1200 + (n - 8) * 400);
        const double hz = 440 * std::pow(2.0, (-1200 + amount * levels[n]) / 1200);
        const double expected = reference.process(source, 2, hz, 1.0, setup.sampleRate);
        assert(std::abs(buffer[n] - expected) < 1e-5);
    }
    assert(processor->setProcessing(false) == kResultOk && component->setActive(false) == kResultOk);
}

#include "gui-smoke.h"

template <typename T>
T* query(FUnknown* object, const TUID iid) {
    void* result = nullptr;
    assert(object->queryInterface(iid, &result) == kResultOk);
    assert(result);
    return static_cast<T*>(result);
}

// Noiseの公開値、サンプル位置での切替、ステレオ出力、停止・保存を標準APIで確認する。
#include "program-smoke.h"

template <typename Sample>
void checkNoise(IComponent* component, IAudioProcessor* processor, IEditController* controller, int32 precision) {
    configureEnvelope(processor, true);
    ProcessSetup setup {kRealtime, precision, 257, 48000.0};
    assert(processor->setupProcessing(setup) == kResultOk);
    assert(component->activateBus(kAudio, kOutput, 0, true) == kResultOk);
    assert(component->setActive(true) == kResultOk);
    assert(processor->setProcessing(true) == kResultOk);
    Points wave, unison, detune, width;
    wave.values = {{0, .5}, {1, 1.0}}; // 矩形波からNoiseへ。
    unison.id = unisonId; unison.values = {{0, 1.0}};
    detune.id = detuneId; detune.values = {{0, 0.0}};
    width.id = 0x57494454; width.values = {{0, 1.0}};
    Changes changes; changes.queues = {&wave, &unison, &detune, &width};
    Events events; events.values = {note(true, 0, 1)};
    std::array<Sample, 257> left {}, right {};
    Sample* channels[] = {left.data(), right.data()};
    AudioBusBuffers bus; bus.numChannels = 2;
    if constexpr (std::is_same_v<Sample, float>) bus.channelBuffers32 = channels;
    else bus.channelBuffers64 = channels;
    ProcessData data; data.symbolicSampleSize = precision; data.numSamples = 257;
    data.numOutputs = 1; data.outputs = &bus; data.inputEvents = &events; data.inputParameterChanges = &changes;
    assert(processor->process(data) == kResultOk);
    assert(left[0] == Sample(.5) && left == right && bus.silenceFlags == 0);
    for (int i = 1; i < 257; ++i) assert(std::isfinite(left[i]) && std::abs(left[i]) <= .5 * std::sqrt(3.0));
    StateStream saved; assert(component->getState(&saved) == kResultOk && saved.bytes[8] == 4);
    assert(controller->setComponentState(&saved) == kResultOk);
    assert(controller->getParamNormalized(waveformId) == 1.0);
    saved.position = 0; assert(component->setState(&saved) == kResultOk);
    events.values.clear(); wave.values.clear(); unison.values.clear(); width.values.clear();
    detune.values = {{0, .25}};
    assert(processor->process(data) == kResultOk);
    assert(left != right); // Detune>0で複数のノイズ源を左右へ配置する。
    for (int i = 0; i < 257; ++i) assert(std::isfinite(left[i]) && std::isfinite(right[i]));
    detune.values.clear(); events.values = {note(false, 1, 1)};
    assert(processor->process(data) == kResultOk);
    for (int i = 1; i < 257; ++i) assert(left[i] == 0 && right[i] == 0);
    events.values.clear();
    assert(processor->process(data) == kResultOk && bus.silenceFlags == 3);
    assert(processor->setProcessing(false) == kResultOk);
    assert(component->setActive(false) == kResultOk);
}

void checkStateReload(IPluginFactory* factory, const TUID cid) {
    // 全31設定を変更して保存・破棄し、新規インスタンスへ復元する。GUIは開かない。
    const std::array<double, 31> plain {3, 25, 15, 250, 0.4567, 350.25, 7, 25.125, 32, 6, 2400.125, 2.5125, 2, -12.345, 25, 15, 250, 0.4567, 350.25, -1234.125, 62.5, -35.25, -3.7, 2, -1234.5, -8.1, 3, 1599.75, -std::numeric_limits<double>::infinity(), 37.5, 89.125};
    std::array<double, 31> expected {};
    StateStream saved;
    for (bool restore : {false, true}) {
        void* object = nullptr;
        assert(factory->createInstance(cid, IComponent_iid, &object) == kResultOk);
        auto* component = static_cast<IComponent*>(object);
        auto* processor = query<IAudioProcessor>(component, IAudioProcessor_iid);
        auto* controller = query<IEditController>(component, IEditController_iid);
        assert(component->initialize(factory) == kResultOk);
        if (!restore) {
            std::array<Points, 31> points;
            Changes changes;
            for (size_t i = 0; i < points.size(); ++i) {
                ParameterInfo info {};
                assert(controller->getParameterInfo(static_cast<int32>(i), info) == kResultOk);
                expected[i] = controller->plainParamToNormalized(info.id, plain[i]);
                points[i].id = info.id;
                points[i].values = {{0, expected[i]}};
                changes.queues.push_back(&points[i]);
            }
            ProcessData data;
            data.inputParameterChanges = &changes;
            assert(processor->process(data) == kResultOk);
            assert(component->getState(&saved) == kResultOk);
        } else {
            // setComponentStateの追加呼び出しやprocessより前に、表示が復元されること。
            assert(component->setState(&saved) == kResultOk);
            StateStream own;
            assert(controller->setState(&own) == kResultOk);
            for (size_t i = 0; i < expected.size(); ++i) {
                ParameterInfo info {};
                assert(controller->getParameterInfo(static_cast<int32>(i), info) == kResultOk);
                assert(std::abs(controller->getParamNormalized(info.id) - expected[i]) < 1e-12);
            }
            StateStream restored;
            assert(component->getState(&restored) == kResultOk && restored.bytes == saved.bytes);
        }
        assert(component->terminate() == kResultOk);
        controller->release();
        processor->release();
        assert(component->release() == 0);
    }
}

template <typename Sample>
void checkSilence(IComponent* component, IAudioProcessor* processor, int32 precision) {
    ProcessSetup setup {kRealtime, precision, 257, 48000.0};
    assert(processor->setupProcessing(setup) == kResultOk);
    assert(component->activateBus(kAudio, kOutput, 0, true) == kResultOk);
    assert(component->setActive(true) == kResultOk);
    assert(processor->setProcessing(true) == kResultOk);

    // 前後の番兵で範囲外書き込みを検知。両出力が同じ領域を指すケースも試す。
    for (bool alias : {false, true}) {
        for (int32 length : {1, 7, 256, 257}) {
            std::array<Sample, 259> left, right;
            left.fill(Sample(0.75));
            right.fill(Sample(0.75));
            Sample* channels[] = {left.data() + 1, alias ? left.data() + 1 : right.data() + 1};
            AudioBusBuffers bus;
            bus.numChannels = 2;
            if constexpr (std::is_same_v<Sample, float>) bus.channelBuffers32 = channels;
            else bus.channelBuffers64 = channels;
            ProcessData data;
            data.symbolicSampleSize = precision;
            data.numSamples = length;
            data.numOutputs = 1;
            data.outputs = &bus;
            assert(processor->process(data) == kResultOk);
            assert(bus.silenceFlags == 3);
            for (auto* channel : channels) {
                assert(channel[-1] == Sample(0.75));
                for (int32 i = 0; i < length; ++i) assert(channel[i] == Sample(0));
                assert(channel[length] == Sample(0.75));
            }
            data.numSamples = 258; // 最大長を超える要求は書き込み前に拒否。
            assert(processor->process(data) == kInvalidArgument);
        }
    }

    // 非アクティブバスのnullバッファを参照しない。
    assert(component->activateBus(kAudio, kOutput, 0, false) == kResultOk);
    AudioBusBuffers inactive;
    inactive.numChannels = 2;
    ProcessData data;
    data.symbolicSampleSize = precision;
    data.numSamples = 7;
    data.numOutputs = 1;
    data.outputs = &inactive;
    assert(processor->process(data) == kResultOk);
    assert(processor->setProcessing(false) == kResultOk);
    assert(component->setActive(false) == kResultOk);
    assert(component->setActive(false) == kResultOk);
    // 停止中のゼロサンプルフラッシュ。
    ProcessData flush;
    flush.symbolicSampleSize = precision;
    assert(processor->process(flush) == kResultOk);
}

#ifdef _WIN32
int wmain(int argc, wchar_t** argv) {
    assert(argc == 3 || argc == 4);
    HMODULE module = LoadLibraryW(argv[1]);
    if (!module) { std::cerr << "LoadLibrary failed: " << GetLastError() << '\n'; return 1; }
    const auto entry = reinterpret_cast<bool (PLUGIN_API*)()>(GetProcAddress(module, "InitDll"));
    const auto exit = reinterpret_cast<bool (PLUGIN_API*)()>(GetProcAddress(module, "ExitDll"));
    const auto getFactory = reinterpret_cast<IPluginFactory* (PLUGIN_API*)()>(GetProcAddress(module, "GetPluginFactory"));
    assert(entry && exit && getFactory && entry());
    const std::wstring wideVersion(argv[2]);
    const std::string expectedVersion(wideVersion.begin(), wideVersion.end());
#else
int main(int argc, char** argv) {
    assert(argc == 3 || argc == 4);
    void* module = dlopen(argv[1], RTLD_NOW | RTLD_LOCAL);
    if (!module) { std::cerr << dlerror() << '\n'; return 1; }
    const auto entry = reinterpret_cast<bool (*)(void*)>(dlsym(module, "ModuleEntry"));
    const auto exit = reinterpret_cast<bool (*)()>(dlsym(module, "ModuleExit"));
    const auto getFactory = reinterpret_cast<IPluginFactory* (*)()>(dlsym(module, "GetPluginFactory"));
    assert(entry && exit && getFactory && entry(module));
    const std::string expectedVersion(argv[2]);
#endif
    auto* factory = getFactory();
    auto* factory2 = query<IPluginFactory2>(factory, IPluginFactory2_iid);
    auto* factoryIdentity = query<FUnknown>(factory2, FUnknown_iid);
    assert(factoryIdentity == factory);
    factoryIdentity->release();
    PFactoryInfo vendor;
    assert(factory->getFactoryInfo(&vendor) == kResultOk);
    assert(std::strcmp(vendor.vendor, "sample-vsti") == 0);
    assert(factory->countClasses() == 1);
    PClassInfo2 info;
    assert(factory2->getClassInfo2(0, &info) == kResultOk);
    assert(std::strcmp(info.subCategories, "Instrument|Synth") == 0);
    assert(std::strcmp(info.version, expectedVersion.c_str()) == 0);
    assert(std::strcmp(info.sdkVersion, kVstVersionString) == 0);
    const TUID expectedClassId = INLINE_UID(0x9ad1ae8c, 0xb51b4aac, 0xa13c85c0, 0xb023ab4d);
    assert(std::memcmp(info.cid, expectedClassId, sizeof(TUID)) == 0);
    void* invalid = factory;
    const TUID unknown = {};
    assert(factory->queryInterface(unknown, &invalid) == kNoInterface && invalid == nullptr);
    assert(factory2->getClassInfo2(1, &info) == kInvalidArgument);
    factory2->release();

    checkStateReload(factory, info.cid);
    checkPrograms<float>(factory, info.cid, kSample32);
    checkPrograms<double>(factory, info.cid, kSample64);
    for (int iteration = 0; iteration < 100; ++iteration) {
        void* result = nullptr;
        assert(factory->createInstance(info.cid, IComponent_iid, &result) == kResultOk);
        auto* component = static_cast<IComponent*>(result);
        auto* processor = query<IAudioProcessor>(component, IAudioProcessor_iid);
        auto* controller = query<IEditController>(component, IEditController_iid);
        auto* requirements = query<IProcessContextRequirements>(processor, IProcessContextRequirements_iid);
        assert(requirements->getProcessContextRequirements() == 0);
        requirements->release();
        auto* identity = query<FUnknown>(controller, FUnknown_iid);
        assert(identity == component);
        identity->release();
        // このプラグインはホストのサービスを使わないため、有効なFUnknownだけを渡す。
        assert(component->initialize(factory) == kResultOk);
        assert(component->getBusCount(kAudio, kInput) == 0);
        assert(component->getBusCount(kAudio, kOutput) == 1);
        assert(component->getBusCount(kEvent, kInput) == 1);
        BusInfo bus;
        assert(component->getBusInfo(kAudio, kOutput, 0, bus) == kResultOk);
        assert(bus.channelCount == 2 && bus.name[0] == u'S');
        SpeakerArrangement output = SpeakerArr::kStereo;
        assert(processor->setBusArrangements(nullptr, 0, &output, 1) == kResultOk);
        assert(controller->getParameterCount() == 32);
        checkViewContract(controller);
        assert(controller->setComponentHandler(nullptr) == kResultOk);
        assert(processor->getLatencySamples() == 0 && processor->getTailSamples() == kInfiniteTail);
        checkSilence<float>(component, processor, kSample32);
        checkSilence<double>(component, processor, kSample64);
        if (iteration == 35) checkFm<float>(component, processor, controller, kSample32);
        if (iteration == 36) checkFm<double>(component, processor, controller, kSample64);
        if (iteration == 33) checkThreeOscillators<float>(component, processor, controller, kSample32);
        if (iteration == 34) checkThreeOscillators<double>(component, processor, controller, kSample64);
        if (iteration == 31) checkNoise<float>(component, processor, controller, kSample32);
        if (iteration == 32) checkNoise<double>(component, processor, controller, kSample64);
        if (iteration == 15) checkEnvelope<float>(component, processor, controller, kSample32);
        if (iteration == 16) checkEnvelope<double>(component, processor, controller, kSample64);
        if (iteration == 17) checkUnison<float>(component, processor, controller, kSample32);
        if (iteration == 18) checkUnison<double>(component, processor, controller, kSample64);
        if (iteration == 19) checkPolyphony<float>(component, processor, controller, kSample32, kRealtime);
        if (iteration == 20) checkPolyphony<double>(component, processor, controller, kSample64, kOffline);
        if (iteration == 21) checkFilter<float>(component, processor, controller, kSample32);
        if (iteration == 22) checkFilter<double>(component, processor, controller, kSample64);
        if (iteration == 23) checkPhase<float>(component, processor, controller, kSample32);
        if (iteration == 24) checkPhase<double>(component, processor, controller, kSample64);
        if (iteration == 25) checkVolume<float>(component, processor, controller, kSample32);
        if (iteration == 26) checkVolume<double>(component, processor, controller, kSample64);
        if (iteration == 27) checkCutoffEnvelope<float>(component, processor, controller, kSample32);
        if (iteration == 28) checkCutoffEnvelope<double>(component, processor, controller, kSample64);
        if (iteration == 29) checkPanning<float>(component, processor, controller, kSample32);
        if (iteration == 30) checkPanning<double>(component, processor, controller, kSample64);
        if (iteration < 12) {
            const double rate = std::array<double, 3> {44100.0, 48000.0, 96000.0}[iteration % 3];
            if (iteration % 2 == 0) checkNotes<float>(component, processor, kSample32, rate, iteration % 3, iteration < 6);
            else checkNotes<double>(component, processor, kSample64, rate, iteration % 3, iteration < 6);
        }
        if (iteration == 12) checkParameters<float>(component, processor, controller, kSample32, kRealtime);
        if (iteration == 13) checkParameters<double>(component, processor, controller, kSample64, kOffline);
        if (iteration == 14 && argc == 4) checkGui(component, processor, controller);
        assert(component->terminate() == kResultOk);
        component->release();
        controller->release();
        assert(processor->release() == 0);
    }
    assert(factory->release() == 0);
    assert(exit());
#ifdef _WIN32
    assert(FreeLibrary(module));
#else
    assert(dlclose(module) == 0);
#endif
    std::cout << "Verified plug-in version: " << expectedVersion << '\n';
    std::cout << "ABI smoke test passed (100 instances, float32/64, notes, phase continuity, waveform automation, state, silence, lifecycle)\n";
}
