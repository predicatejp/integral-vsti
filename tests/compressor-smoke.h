// Copyright (C) 2026 Yusuke Oda
// SPDX-License-Identifier: AGPL-3.0-only

// 3段の静特性、128番以降のオートメーション、可変長ブロックを実出力で照合する。
template <typename Sample>
void checkCompressor(IComponent* component, IAudioProcessor* processor, IEditController* controller, int32 precision) {
    for (double rate : {44100., 48000., 96000.}) {
        auto render = [&](int slots, bool split, bool automate) {
            ProcessSetup setup {split ? kOffline : kRealtime, precision, 257, rate};
            assert(processor->setupProcessing(setup) == kResultOk);
            std::array<Points, 26> points;
            Changes changes;
            for (int i = 0; i < 26; ++i) {
                const int index = i == 0 ? 130 : i == 1 ? 12 : i < 5 ? 40+i : 104+i;
                const double plain = i < 2 ? i : i < 5 ? (i-2 < slots ? 2 : 0) :
                    std::array<double,7>{-18,4,0,0,0,0,100}[(i-5)%7];
                ParameterInfo info; assert(controller->getParameterInfo(index, info) == kResultOk);
                points[i].id = info.id;
                points[i].values = {{0, controller->plainParamToNormalized(info.id, plain)}};
                changes.queues.push_back(&points[i]);
            }
            ProcessData data {}; data.symbolicSampleSize = precision; data.processMode = setup.processMode;
            data.inputParameterChanges = &changes;
            assert(processor->process(data) == kResultOk);
            assert(component->activateBus(kAudio, kOutput, 0, true) == kResultOk);
            assert(component->activateBus(kEvent, kInput, 0, true) == kResultOk);
            assert(component->setActive(true) == kResultOk && processor->setProcessing(true) == kResultOk);
            std::array<Sample,257> left {}, right {};
            Sample* channels[] = {left.data(), right.data()};
            AudioBusBuffers bus {}; bus.numChannels = 2;
            if constexpr (std::is_same_v<Sample,float>) bus.channelBuffers32 = channels;
            else bus.channelBuffers64 = channels;
            data.numOutputs = 1; data.outputs = &bus;
            Events events; events.values = {note(true,0,1,69)};
            Changes automation;
            for (int i = 23; i < 26; ++i) {
                points[i].values = {{63, i == 24 ? .25 : .5}};
                automation.queues.push_back(&points[i]);
            }
            std::vector<std::array<Sample,2>> result;
            for (int n = 0; n < 2048;) {
                data.numSamples = std::min(2048-n, split && n > 0 ? (n%3 == 0 ? 1 : 63) : 257);
                data.inputParameterChanges = automate && n == 0 ? &automation : nullptr;
                data.inputEvents = n == 0 ? &events : nullptr;
                assert(processor->process(data) == kResultOk);
                for (int i = 0; i < data.numSamples; ++i) result.push_back({left[i],right[i]});
                n += data.numSamples;
            }
            StateStream saved; assert(component->getState(&saved) == kResultOk);
            assert(controller->setComponentState(&saved) == kResultOk);
            if (automate) for (int i = 23; i < 26; ++i)
                assert(controller->getParamNormalized(points[i].id) == (i == 24 ? .25 : .5));
            assert(processor->getLatencySamples() == 3 + static_cast<uint32>(std::ceil(rate*.005)));
            assert(processor->setProcessing(false) == kResultOk && component->setActive(false) == kResultOk);
            return result;
        };
        const auto dry = render(0,false,false);
        for (int slots = 1; slots <= 3; ++slots) for (bool automate : {false,true}) {
            const auto wet = render(slots,false,automate);
            const auto split = render(slots,true,automate);
            for (size_t n = 0; n < wet.size(); ++n) for (int ch = 0; ch < 2; ++ch) {
                assert(wet[n][ch] == split[n][ch]);
                double expected = dry[n][ch];
                for (int slot = 0; slot < slots; ++slot) {
                    const double ramp = automate && slot == 2 ? std::clamp((double(n)-processor->getLatencySamples()+1)/64,0.,1.) : 0;
                    const double knee = 12*ramp, makeup = 6*ramp, mix = 1-.5*ramp;
                    const double over = 20*std::log10(std::abs(expected)) + 18;
                    const double reduction = over <= -knee/2 ? 0 :
                        .75*(knee > 0 && over < knee/2 ? std::pow(over+knee/2,2)/(2*knee) : over);
                    expected *= 1-mix + mix*std::pow(10.,(makeup-reduction)/20);
                }
                assert(std::isfinite(wet[n][ch]) && std::abs(wet[n][ch]-expected) < 1e-6);
            }
        }
    }
}
