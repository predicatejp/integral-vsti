// Copyright (C) 2026 Yusuke Oda
// SPDX-License-Identifier: AGPL-3.0-only

// ホストが再有効化しなくても、Rateは次の音声処理へ反映される。
class RateHandler final : public IComponentHandler {
public:
    IAudioProcessor* processor;
    const std::thread::id thread = std::this_thread::get_id();
    uint32 refs = 1;
    unsigned restarts = 0;
    uint32 reportedLatency = 0;
    explicit RateHandler(IAudioProcessor* p) : processor(p) {}
    tresult PLUGIN_API queryInterface(const TUID, void** out) override { *out = nullptr; return kNoInterface; }
    uint32 PLUGIN_API addRef() override { return ++refs; }
    uint32 PLUGIN_API release() override { return --refs; }
    tresult PLUGIN_API beginEdit(ParamID) override { assert(false); return kResultFalse; }
    tresult PLUGIN_API performEdit(ParamID, ParamValue) override { assert(false); return kResultFalse; }
    tresult PLUGIN_API endEdit(ParamID) override { assert(false); return kResultFalse; }
    tresult PLUGIN_API restartComponent(int32 flags) override {
        assert(thread == std::this_thread::get_id() && flags == kLatencyChanged);
        // 遅延通知には応答するが、停止・再有効化は行わない。
        reportedLatency = processor->getLatencySamples();
        ++restarts;
        return kResultOk;
    }
};

template <typename Sample>
void checkOversampling(IComponent* component, IAudioProcessor* processor,
                       IEditController* controller, int32 precision) {
    ParameterInfo rate, program;
    assert(controller->getParameterInfo(132, rate) == kResultOk);
    assert(rate.id == 0x46585254 && rate.stepCount == 2 && rate.defaultNormalizedValue == 0);
    assert(rate.flags == ParameterInfo::kIsList);
    assert(controller->getParameterInfo(133, program) == kResultOk);
    Points preset; preset.id = program.id; preset.values = {{0, 0}};
    Changes changes; changes.queues = {&preset};
    std::array<Points, 6> settings;
    const std::array<int, 6> indices {0, 12, 42, 31, 32, 33};
    const std::array<double, 6> plain {3, 1, 4, 0, 48, 100}; // Saw、Zero、Soft、Gain最大。
    for (size_t i = 0; i < settings.size(); ++i) {
        ParameterInfo info; assert(controller->getParameterInfo(indices[i], info) == kResultOk);
        settings[i].id = info.id;
        settings[i].values = {{0, controller->plainParamToNormalized(info.id, plain[i])}};
        changes.queues.push_back(&settings[i]);
    }
    ProcessData data {}; data.symbolicSampleSize = precision; data.inputParameterChanges = &changes;
    assert(processor->process(data) == kResultOk);
    ProcessSetup setup {kRealtime, precision, 257, 48000.};
    assert(processor->setupProcessing(setup) == kResultOk);
    assert(component->activateBus(kAudio, kOutput, 0, true) == kResultOk);
    assert(component->activateBus(kEvent, kInput, 0, true) == kResultOk);
    assert(component->setActive(true) == kResultOk && processor->setProcessing(true) == kResultOk);
    RateHandler handler(processor);
    assert(controller->setComponentHandler(&handler) == kResultOk);
    std::array<Sample, 257> left {}, right {};
    Sample* channels[] = {left.data(), right.data()};
    AudioBusBuffers bus {}; bus.numChannels = 2;
    if constexpr (std::is_same_v<Sample, float>) bus.channelBuffers32 = channels;
    else bus.channelBuffers64 = channels;
    data.numOutputs = 1; data.outputs = &bus; data.inputParameterChanges = nullptr;
    StateStream saved;
    constexpr std::array<uint32, 3> latency {243, 305, 308};
    for (int16 pitch = 108; pitch <= 120; ++pitch) {
        std::array<double, 3> rms {};
        for (int mode : {0, 1, 2}) {
            const auto before = handler.restarts;
            assert(controller->setParamNormalized(rate.id, mode / 2.) == kResultOk);
            assert(processor->getLatencySamples() == latency[mode]);
            if (mode != 0 || pitch != 108) {
                assert(handler.restarts == before + 1 && handler.reportedLatency == latency[mode]);
            }
            const auto notified = handler.restarts;
            assert(controller->setParamNormalized(rate.id, mode / 2.) == kResultOk);
            assert(handler.restarts == notified);
            // 設定は次のprocessが来る前から保存できる。
            if (mode == 2 && pitch == 120) assert(component->getState(&saved) == kResultOk);
            Events events; events.values = {note(true, 0, pitch, pitch)};
            double energy = 0;
            for (int n = 0; n < 8192;) {
                data.numSamples = std::min(8192 - n, n % 2 ? 257 : 31);
                // x1→x2→x4の切替中はノートを保持し、追加Note Onを送らない。
                data.inputEvents = mode == 0 && n == 0 ? &events : nullptr;
                assert(processor->process(data) == kResultOk);
                for (int i = 0; i < data.numSamples; ++i) {
                    assert(std::isfinite(left[i]) && left[i] == right[i]);
                    if (n + i >= 1024) energy += double(left[i]) * left[i];
                }
                n += data.numSamples;
            }
            rms[mode] = std::sqrt(energy / (8192 - 1024));
            assert(rms[mode] > .7 && rms[mode] < 1.1);
        }
        assert(rms[1] - rms[0] > .01 && rms[2] - rms[0] > .01);
    }
    assert(controller->setParamNormalized(rate.id, 0) == kResultOk);
    const auto before = handler.restarts;
    assert(component->setState(&saved) == kResultOk);
    saved.position = 0;
    assert(controller->setComponentState(&saved) == kResultOk);
    assert(handler.restarts == before + 1 && processor->getLatencySamples() == 308);
    assert(controller->getParamNormalized(rate.id) == 1);
    assert(controller->setParamNormalized(rate.id, 0) == kResultOk);
    assert(processor->getLatencySamples() == 243);
    assert(controller->setComponentHandler(nullptr) == kResultOk && handler.refs == 1);
    assert(processor->setProcessing(false) == kResultOk && component->setActive(false) == kResultOk);
}
