// Copyright (C) 2026 Yusuke Oda
// SPDX-License-Identifier: AGPL-3.0-only

// EQ後・Master前の上限制御、Off/Onの時刻、純遅延とレート変更を公式ABIで検証する。
template <typename Sample>
void checkLimiter(IComponent* component, IAudioProcessor* processor, IEditController* controller, int32 precision) {
    ParameterInfo info;
    assert(controller->getParameterInfo(108, info) == kResultOk);
    assert(info.id == 0x4c490000 && info.stepCount == 1 && info.defaultNormalizedValue == 0);
    for (double rate : {44100., 48000., 88200., 96000., 192000.}) {
        auto render = [&](bool enabled, double volume, bool split, bool automate) {
            ProcessSetup setup {split ? kOffline : kRealtime, precision, 257, rate};
            assert(processor->setupProcessing(setup) == kResultOk);
            const auto latency = 3 + static_cast<uint32>(std::ceil(rate * .005));
            assert(processor->getLatencySamples() == latency);
            std::array<Points, 9> points;
            Changes changes;
            const std::array<int, 9> indices {132, 12, 8, 34, 35, 36, 21, 13, 108};
            const std::array<double, 9> plain {0, 1, 4, 1, 20000, 12, 25, volume, double(enabled)};
            for (size_t i = 0; i < points.size(); ++i) {
                ParameterInfo parameter; assert(controller->getParameterInfo(indices[i], parameter) == kResultOk);
                points[i].id = parameter.id;
                points[i].values = {{0, controller->plainParamToNormalized(parameter.id, plain[i])}};
                changes.queues.push_back(&points[i]);
            }
            ProcessData data {}; data.symbolicSampleSize = precision; data.processMode = setup.processMode;
            data.inputParameterChanges = &changes;
            assert(processor->process(data) == kResultOk);
            assert(component->activateBus(kAudio, kOutput, 0, true) == kResultOk);
            assert(component->activateBus(kEvent, kInput, 0, true) == kResultOk);
            assert(component->setActive(true) == kResultOk && processor->setProcessing(true) == kResultOk);
            std::array<Sample, 257> left {}, right {};
            Sample* channels[] = {left.data(), right.data()};
            AudioBusBuffers bus {}; bus.numChannels = 2;
            if constexpr (std::is_same_v<Sample, float>) bus.channelBuffers32 = channels;
            else bus.channelBuffers64 = channels;
            data.numOutputs = 1; data.outputs = &bus;
            Events events; for (int id = 0; id < 4; ++id) events.values.push_back(note(true, 0, id, 69));
            Points toggle; toggle.id = info.id;
            Changes automation; automation.queues = {&toggle};
            std::vector<std::array<Sample, 2>> result;
            for (int n = 0; n < 4096;) {
                data.numSamples = std::min(4096 - n, split ? (n == 0 ? 257 : n % 3 == 0 ? 1 : 63) : 257);
                // 2ブロック目の最初の11サンプルまでOffを保持する。
                toggle.values = {{11, 1}};
                data.inputParameterChanges = automate && n == 257 ? &automation : nullptr;
                data.inputEvents = n == 0 ? &events : nullptr;
                assert(processor->process(data) == kResultOk);
                for (int i = 0; i < data.numSamples; ++i) {
                    assert(std::isfinite(left[i]) && std::isfinite(right[i]));
                    result.push_back({left[i], right[i]});
                }
                n += data.numSamples;
            }
            assert(processor->getLatencySamples() == latency);
            assert(processor->setProcessing(false) == kResultOk && component->setActive(false) == kResultOk);
            return result;
        };
        const auto dry = render(false, 0, false, false);
        const auto limited = render(true, 0, false, false);
        const auto split = render(true, 0, true, false);
        const auto amplified = render(true, 6, false, false);
        const auto toggle = render(false, 0, false, true);
        double dryPeak = 0, peak = 0;
        for (size_t n = 0; n < dry.size(); ++n) for (int ch = 0; ch < 2; ++ch) {
            dryPeak = std::max(dryPeak, std::abs(double(dry[n][ch])));
            peak = std::max(peak, std::abs(double(limited[n][ch])));
            assert(limited[n][ch] == split[n][ch]);
            assert(std::abs(amplified[n][ch] - limited[n][ch] * std::pow(10., .3)) < 1e-6);
            assert(toggle[n][ch] == (n < 268 ? dry[n][ch] : limited[n][ch]));
        }
        assert(dryPeak > 8 && peak > .9 && peak <= 1.000001);
    }
}
