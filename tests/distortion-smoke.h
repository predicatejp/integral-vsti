// Copyright (C) 2026 Yusuke Oda
// SPDX-License-Identifier: AGPL-3.0-only

// 多声音の共有Clipperをステレオで処理し、フラッシュ・実時間切替・可変ブロック・停止を検証する。
template <typename Sample>
void checkDistortionBlocks(IComponent* component, IAudioProcessor* processor, IEditController* controller, int32 precision) {
    String128 text {};
    assert(controller->getParamStringByValue(0x4f534d50, 0, text) == kInvalidArgument);
    assert(controller->getParamStringByValue(0x41444141, 0, text) == kInvalidArgument);
    for (double rate : {44100.0, 48000.0, 96000.0}) for (int type : {1, 2}) for (int mask = 0; mask < 8; ++mask) {
        std::vector<Sample> reference;
        for (bool split : {false, true}) {
            configureEnvelope(processor, true);
            ProcessSetup setup {split ? kOffline : kRealtime, precision, 257, rate};
            assert(processor->setupProcessing(setup) == kResultOk);
            Points phase, clip, gain, poly, pan, slot;
            slot.id = 0x45463154; slot.values = {{0, double(mask & 1)}};
            phase.id = phaseId; phase.values = {{0, .5}};
            clip.id = 0x43545950; clip.values = {{0, type / 2.0}};
            gain.id = 0x43474149; gain.values = {{0, .25}};
            poly.id = polyphonyId; poly.values = {{0, 3.0 / 31.0}};
            pan.id = 0x4d50414e; pan.values = {{0, .625}};
            Changes changes; changes.queues = {&phase, &clip, &gain, &poly, &pan, &slot};
            std::array<Points, 8> extra;
            const std::array<int, 8> indices {43, 45, 46, 47, 44, 48, 49, 50};
            const std::array<double, 8> values {double((mask >> 1) & 1), 1, .125, .75, double((mask >> 2) & 1), .5, .375, .5};
            for (size_t i = 0; i < extra.size(); ++i) {
                ParameterInfo info; assert(controller->getParameterInfo(indices[i], info) == kResultOk);
                extra[i].id = info.id; extra[i].values = {{0, values[i]}};
                changes.queues.push_back(&extra[i]);
            }
            ProcessData data {}; data.symbolicSampleSize = precision;
            data.inputParameterChanges = &changes;
            assert(processor->process(data) == kResultOk); // 停止中の0サンプル更新。
            assert(processor->getLatencySamples() == 3);
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
            Events events; events.values = {note(true, 0, 1, 60), note(true, 0, 2, 64), note(true, 0, 3, 67), note(true, 0, 4, 72)};
            std::vector<Sample> actual;
            bool stereo = false;
            for (int n = 0; n < 1024;) {
                data.numSamples = std::min(1024 - n, split ? (n % 3 == 0 ? 1 : 63) : 257);
                data.inputEvents = n == 0 ? &events : nullptr;
                assert(processor->process(data) == kResultOk);
                for (int i = 0; i < data.numSamples; ++i) {
                    assert(std::isfinite(left[i]) && std::isfinite(right[i]));
                    stereo |= left[i] != right[i];
                    actual.push_back(left[i]);
                    actual.push_back(right[i]);
                }
                n += data.numSamples;
            }
            if (split) assert(actual == reference);
            else reference = actual;
            assert(stereo);
            assert(std::any_of(actual.begin(), actual.end(), [](Sample x) {return std::abs(x) > .1;}));
            // 保持中のノートのままClipperを切り替え、遅延申告と出力の有限性を確認する。
            clip.values = {{0, (3 - type) / 2.0}}; changes.queues = {&clip};
            data.inputEvents = nullptr; data.inputParameterChanges = &changes; data.numSamples = 257;
            assert(processor->process(data) == kResultOk && processor->getLatencySamples() == 3);
            for (int i = 0; i < 257; ++i) assert(std::isfinite(left[i]) && std::isfinite(right[i]));
            assert(processor->setProcessing(false) == kResultOk);
            assert(processor->setProcessing(true) == kResultOk);
            data.inputParameterChanges = nullptr; data.numSamples = 7;
            assert(processor->process(data) == kResultOk);
            for (int i = 0; i < data.numSamples; ++i) assert(left[i] == 0 && right[i] == 0);
            assert(processor->setProcessing(false) == kResultOk && component->setActive(false) == kResultOk);
        }
    }
}
