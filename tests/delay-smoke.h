// Copyright (C) 2026 Yusuke Oda
// SPDX-License-Identifier: AGPL-3.0-only

// テンポ要求・有効フラグ・欠測保持と、3スロットの直列遅延を公式ABIで検証する。
template <typename Sample>
void checkDelay(IComponent* component, IAudioProcessor* processor, IEditController* controller, int32 precision) {
    std::vector<Sample> dry;
    for (int trial = 0; trial < 11; ++trial) {
        ProcessSetup setup {trial % 2 ? kOffline : kRealtime, precision, 257, 48000.0};
        assert(processor->setupProcessing(setup) == kResultOk);
        const int slots = trial == 0 ? 0 : trial < 5 ? 1 : trial < 7 ? trial - 3 : 1;
        std::array<Points, 15> points;
        Changes changes;
        const std::array<int, 15> indices {132, 12, 42, 51, 52, 53, 43, 54, 55, 56, 44, 57, 58, 59, 60};
        const std::array<double, 15> values {0, .5, slots > 0 ? 3.0/6.0 : 0.0, 0, 1, 1,
            slots > 1 ? 3.0/6.0 : 0.0, 0, 0, 1, slots > 2 ? 3.0/6.0 : 0.0, 0, 0, 1, trial < 7 ? 0.0 : (trial - 7) / 3.0};
        for (size_t i = 0; i < points.size(); ++i) {
            ParameterInfo info; assert(controller->getParameterInfo(indices[i], info) == kResultOk);
            points[i].id = info.id; points[i].values = {{0, values[i]}};
            changes.queues.push_back(&points[i]);
        }
        ProcessContext context {};
        context.state = trial == 3 ? 0 : ProcessContext::kTempoValid;
        context.tempo = trial <= 1 ? 120 : trial == 2 ? 150 : trial == 3 ? 180 : trial == 4 ? std::nan("") : trial < 7 ? 75 : 120;
        ProcessData data {}; data.symbolicSampleSize = precision;
        data.inputParameterChanges = &changes;
        data.processContext = trial <= 1 ? nullptr : &context;
        assert(processor->process(data) == kResultOk); // 停止中のtempo更新も保持する。
        assert(component->activateBus(kAudio, kOutput, 0, true) == kResultOk);
        assert(component->activateBus(kEvent, kInput, 0, true) == kResultOk);
        assert(component->setActive(true) == kResultOk && processor->setProcessing(true) == kResultOk);
        std::array<Sample, 257> left {}, right {};
        Sample* channels[] = {left.data(), right.data()};
        AudioBusBuffers bus {}; bus.numChannels = 2;
        if constexpr (std::is_same_v<Sample, float>) bus.channelBuffers32 = channels;
        else bus.channelBuffers64 = channels;
        data.numOutputs = 1; data.outputs = &bus; data.processMode = setup.processMode;
        data.inputParameterChanges = nullptr;
        data.processContext = nullptr; // 後続の欠測・再生位置によらずtempoとtailを維持。
        Events events; events.values = {note(true, 0, 1, 69), note(false, 7, 1, 69)};
        std::vector<Sample> actual, actualRight;
        const int delay = trial <= 1 ? 3000 : trial <= 4 ? 2400 : trial < 7 ? 4800 : 3000;
        const int length = (slots + 2) * delay + 64;
        for (int n = 0; n < length;) {
            data.numSamples = std::min(length - n, trial % 2 ? 61 : 257);
            data.inputEvents = n == 0 ? &events : nullptr;
            assert(processor->process(data) == kResultOk);
            for (int i = 0; i < data.numSamples; ++i) {
                assert(std::isfinite(left[i]) && std::isfinite(right[i]));
                actual.push_back(left[i]);
                actualRight.push_back(right[i]);
            }
            n += data.numSamples;
        }
        if (trial == 0) dry = actual;
        else {
            for (int n = 0; n < length; ++n) {
                Sample expected = 0;
                // 最初のDelayだけFeedback=100%。後段は純粋なWet遅延。
                for (int offset = slots * delay; offset <= n; offset += delay)
                    if (n - offset < int(processor->getLatencySamples()) + 16) expected += dry[n - offset];
                const bool leftOnly = trial == 9 ? (n / delay) % 2 == 1 : (n / delay) % 2 == 0;
                assert(std::abs(actual[n] - (trial < 9 || leftOnly ? expected : 0)) < 1e-6);
                assert(std::abs(actualRight[n] - (trial < 9 || !leftOnly ? expected : 0)) < 1e-6);
            }
        }
        assert(processor->getLatencySamples() == 3 + static_cast<uint32>(std::ceil(setup.sampleRate * .005)));
        assert(processor->setProcessing(false) == kResultOk);
        assert(processor->setProcessing(true) == kResultOk);
        data.inputEvents = nullptr; data.numSamples = 257;
        for (int n = 0; n < length; n += 257) {
            assert(processor->process(data) == kResultOk);
            for (int i = 0; i < 257; ++i) assert(left[i] == 0 && right[i] == 0);
        }
        assert(processor->setProcessing(false) == kResultOk && component->setActive(false) == kResultOk);
    }
}
