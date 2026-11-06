// Copyright (C) 2026 Yusuke Oda
// SPDX-License-Identifier: AGPL-3.0-only

// 既存の独立した参照波形にホスト申告の遅延を適用し、最後の区間も排出して検証する。
template <typename Sample>
class LatencyReference {
    IAudioProcessor* processor;
    std::vector<std::array<double, 2>> history;
    size_t position = 0;
    bool absolute;
public:
    explicit LatencyReference(IAudioProcessor* p, bool abs = false, uint32 alreadyDelayed = 0)
        : processor(p), history(p->getLatencySamples() - alreadyDelayed), absolute(abs) { assert(p->getLatencySamples() == 241); }
    void reset() { std::fill(history.begin(), history.end(), std::array<double, 2>{}); position = 0; }
    std::array<double, 2> next(double left, double right) {
        const auto result = history[position];
        history[position] = {left, right}; position = (position + 1) % history.size();
        return result;
    }
    double next(double value) { return next(value, value)[0]; }
    void flush() {
        Sample left = 0, right = 0;
        Sample* channels[] = {&left, &right};
        AudioBusBuffers bus {}; bus.numChannels = 2;
        if constexpr (std::is_same_v<Sample, float>) bus.channelBuffers32 = channels;
        else bus.channelBuffers64 = channels;
        ProcessData data {}; data.symbolicSampleSize = std::is_same_v<Sample, float> ? kSample32 : kSample64;
        data.numSamples = 1; data.numOutputs = 1; data.outputs = &bus;
        for (size_t i = 0; i < history.size(); ++i) {
            assert(processor->process(data) == kResultOk);
            const auto expected = next(0, 0);
            assert(std::abs((absolute ? std::abs(left) : left) - expected[0]) < 2e-5);
            assert(std::abs((absolute ? std::abs(right) : right) - expected[1]) < 2e-5);
            assert(bus.silenceFlags == (uint64(left == 0) | (uint64(right == 0) << 1)));
        }
    }
};
