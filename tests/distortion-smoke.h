// Copyright (C) 2026 Yusuke Oda
// SPDX-License-Identifier: AGPL-3.0-only

// Clipperのフラッシュ・実時間切替と、可変ブロックでも同じ出力になることを検証する。
template <typename Sample>
void checkDistortionBlocks(IComponent* component, IAudioProcessor* processor, IEditController* controller, int32 precision) {
    String128 text {};
    assert(controller->getParamStringByValue(0x4f534d50, 0, text) == kInvalidArgument);
    assert(controller->getParamStringByValue(0x41444141, 0, text) == kInvalidArgument);
    for (double rate : {44100.0, 48000.0, 96000.0}) for (int type : {1, 2}) {
        std::vector<Sample> reference;
        for (bool split : {false, true}) {
            configureEnvelope(processor, true);
            ProcessSetup setup {split ? kOffline : kRealtime, precision, 257, rate};
            assert(processor->setupProcessing(setup) == kResultOk);
            Points phase, clip, gain;
            phase.id = phaseId; phase.values = {{0, .5}};
            clip.id = 0x43545950; clip.values = {{0, type / 2.0}};
            gain.id = 0x43474149; gain.values = {{0, .25}};
            Changes changes; changes.queues = {&phase, &clip, &gain};
            ProcessData data {}; data.symbolicSampleSize = precision;
            data.inputParameterChanges = &changes;
            assert(processor->process(data) == kResultOk); // 停止中の0サンプル更新。
            assert(processor->getLatencySamples() == 1);
            assert(component->activateBus(kAudio, kOutput, 0, true) == kResultOk);
            assert(component->activateBus(kEvent, kInput, 0, true) == kResultOk);
            assert(component->setActive(true) == kResultOk && processor->setProcessing(true) == kResultOk);
            std::array<Sample, 257> left {}, right {};
            Sample* channels[] = {left.data(), right.data()};
            AudioBusBuffers bus {}; bus.numChannels = 2;
            if constexpr (std::is_same_v<Sample, float>) bus.channelBuffers32 = channels;
            else bus.channelBuffers64 = channels;
            data.processMode = setup.processMode; data.numOutputs = 1; data.outputs = &bus;
            data.inputParameterChanges = nullptr;
            Events events; events.values = {note(true, 0, 1)};
            std::vector<Sample> actual;
            for (int n = 0; n < 1024;) {
                data.numSamples = std::min(1024 - n, split ? (n % 3 == 0 ? 1 : 63) : 257);
                data.inputEvents = n == 0 ? &events : nullptr;
                assert(processor->process(data) == kResultOk);
                for (int i = 0; i < data.numSamples; ++i) {
                    assert(std::isfinite(left[i]) && left[i] == right[i]);
                    actual.push_back(left[i]);
                }
                n += data.numSamples;
            }
            if (split) assert(actual == reference);
            else reference = actual;
            assert(std::any_of(actual.begin(), actual.end(), [](Sample x) {return std::abs(x) > .1;}));
            // 保持中のノートのままClipperを切り替え、遅延申告と出力の有限性を確認する。
            clip.values = {{0, (3 - type) / 2.0}}; changes.queues = {&clip};
            data.inputEvents = nullptr; data.inputParameterChanges = &changes; data.numSamples = 257;
            assert(processor->process(data) == kResultOk && processor->getLatencySamples() == 1);
            for (int i = 0; i < 257; ++i) assert(std::isfinite(left[i]) && std::isfinite(right[i]));
            assert(processor->setProcessing(false) == kResultOk && component->setActive(false) == kResultOk);
        }
    }
}
