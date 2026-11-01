// Copyright (C) 2026 Yusuke Oda
// SPDX-License-Identifier: AGPL-3.0-only

// 倍率のフラッシュ・実時間切替と、可変ブロックでも同じ出力になることを検証する。
template <typename Sample>
void checkOversampling(IComponent* component, IAudioProcessor* processor, IEditController* controller, int32 precision) {
    ParameterInfo info {};
    assert(controller->getParameterInfo(34, info) == kResultOk);
    assert(info.id == 0x4f534d50 && info.stepCount == 2 && std::u16string(info.title) == u"Rate");
    ParameterInfo adaaInfo {};
    assert(controller->getParameterInfo(35, adaaInfo) == kResultOk);
    assert(adaaInfo.id == 0x41444141 && adaaInfo.stepCount == 2 && std::u16string(adaaInfo.title) == u"Smooth");
    for (int m = 0; m < 3; ++m) {
        String128 text {};
        assert(controller->getParamStringByValue(adaaInfo.id, m / 2.0, text) == kResultOk);
        assert(std::u16string(text) == (m == 0 ? u"None" : m == 1 ? u"1st" : u"2nd"));
        assert(controller->getParamStringByValue(info.id, m / 2.0, text) == kResultOk);
        assert(std::u16string(text) == (m == 0 ? u"x1" : m == 1 ? u"x2" : u"x4"));
    }
    for (double rate : {44100.0, 48000.0, 96000.0}) for (int m = 0; m < 3; ++m) for (int a = 0; a < 3; ++a) {
        std::vector<Sample> reference;
        for (bool split : {false, true}) {
            configureEnvelope(processor, true);
            ProcessSetup setup {split ? kOffline : kRealtime, precision, 257, rate};
            assert(processor->setupProcessing(setup) == kResultOk);
            Points mode, phase, clip, gain, adaa;
            adaa.id = adaaInfo.id; adaa.values = {{0, a / 2.0}};
            mode.id = info.id; mode.values = {{0, m / 2.0}};
            phase.id = phaseId; phase.values = {{0, .5}};
            clip.id = 0x43545950; clip.values = {{0, .5}};
            gain.id = 0x43474149; gain.values = {{0, .25}};
            Changes changes; changes.queues = {&mode, &phase, &clip, &gain, &adaa};
            ProcessData data {}; data.symbolicSampleSize = precision;
            data.inputParameterChanges = &changes;
            assert(processor->process(data) == kResultOk); // 停止中の0サンプル更新。
            assert(processor->getLatencySamples() == 69);
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
            // 保持中のノートのまま倍率を切り替え、遅延申告と出力の有限性を確認する。
            mode.values = {{0, ((m + 1) % 3) / 2.0}}; adaa.values = {{0, ((a + 1) % 3) / 2.0}}; changes.queues = {&mode, &adaa};
            data.inputEvents = nullptr; data.inputParameterChanges = &changes; data.numSamples = 257;
            assert(processor->process(data) == kResultOk && processor->getLatencySamples() == 69);
            for (int i = 0; i < 257; ++i) assert(std::isfinite(left[i]) && std::isfinite(right[i]));
            assert(processor->setProcessing(false) == kResultOk && component->setActive(false) == kResultOk);
        }
    }
}
