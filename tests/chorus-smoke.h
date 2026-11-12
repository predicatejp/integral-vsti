// Copyright (C) 2026 Yusuke Oda
// SPDX-License-Identifier: AGPL-3.0-only

// 高位のParamIDキュー、Wet/Dry混合、尾部、可変長ブロックと処理停止を検証する。
template <typename Sample>
void checkChorus(IComponent* component, IAudioProcessor* processor, IEditController* controller, int32 precision) {
    for (double rate : {44100.0, 48000.0, 96000.0}) {
        const int length = static_cast<int>(rate);
        auto render = [&](int slots, bool split, bool automate) {
            ProcessSetup setup {split ? kOffline : kRealtime, precision, 257, rate};
            assert(processor->setupProcessing(setup) == kResultOk);
            std::array<Points, 23> points;
            Changes changes;
            const std::array<int, 23> indices {133,12,42,43,44,75,76,77,78,79,80,81,82,83,84,85,86,87,88,89,90,91,92};
            const std::array<double, 23> plain {0,1,slots>=3?1.:0.,slots>=2?1.:0.,slots>=1?1.:0.,
                2.5,.5,.7,.1,50,100,2.5,.5,.7,-.1,-50,100,2.5,.5,.7,.1,50,100};
            for (size_t i=0;i<points.size();++i) {
                ParameterInfo info; assert(controller->getParameterInfo(indices[i],info)==kResultOk);
                points[i].id=info.id;
                points[i].values={{0,controller->plainParamToNormalized(info.id,plain[i])}};
                changes.queues.push_back(&points[i]);
            }
            ProcessData data {}; data.symbolicSampleSize=precision; data.inputParameterChanges=&changes;
            assert(processor->process(data)==kResultOk);
            assert(component->activateBus(kAudio,kOutput,0,true)==kResultOk);
            assert(component->activateBus(kEvent,kInput,0,true)==kResultOk);
            assert(component->setActive(true)==kResultOk && processor->setProcessing(true)==kResultOk);
            std::array<Sample,257> left {},right {};
            Sample* channels[]={left.data(),right.data()};
            AudioBusBuffers bus {};bus.numChannels=2;
            if constexpr(std::is_same_v<Sample,float>) bus.channelBuffers32=channels;
            else bus.channelBuffers64=channels;
            data.numOutputs=1;data.outputs=&bus;data.processMode=setup.processMode;
            Events events;events.values={note(true,0,1,69),note(false,7,1,69)};
            Points mix;mix.id=0x43334d58;mix.values={{0,0},{255,1}};
            Changes automation;automation.queues={&mix};
            std::vector<std::array<Sample,2>> result;
            for(int n=0;n<length;) {
                data.numSamples=std::min(length-n,split?(n==0?257:n%3==0?1:63):257);
                data.inputEvents=n==0?&events:nullptr;
                data.inputParameterChanges=automate && n==0?&automation:nullptr;
                assert(processor->process(data)==kResultOk);
                for(int i=0;i<data.numSamples;++i) {
                    assert(std::isfinite(left[i]) && std::isfinite(right[i]));
                    result.push_back({left[i],right[i]});
                }
                n+=data.numSamples;
            }
            assert(processor->getLatencySamples()==3 + static_cast<uint32>(std::ceil(rate * .005)));
            assert(processor->setProcessing(false)==kResultOk && processor->setProcessing(true)==kResultOk);
            data.inputEvents=nullptr;data.inputParameterChanges=nullptr;data.numSamples=257;
            for(int n=0;n<length;n+=257) {
                assert(processor->process(data)==kResultOk);
                for(int i=0;i<257;++i) assert(left[i]==0 && right[i]==0);
            }
            assert(processor->setProcessing(false)==kResultOk && component->setActive(false)==kResultOk);
            return result;
        };
        const auto dry=render(0,false,false);
        for(int slots=1;slots<=3;++slots) {
            const auto wet=render(slots,false,false);
            const auto split=render(slots,true,false);
            double tail=0,stereo=0;
            for(int n=0;n<length;++n) for(int ch=0;ch<2;++ch) {
                assert(std::abs(wet[n][ch]-split[n][ch])<1e-6);
                if(n>100) tail+=wet[n][ch]*wet[n][ch];
                stereo+=std::abs(wet[n][0]-wet[n][1]);
            }
            assert(tail>1e-5 && stereo>1e-3);
            if(slots==1) {
                const auto ramp=render(1,true,true);
                for(int n=0;n<length;++n) {
                    // Mixを適用した時刻から系列末尾の申告遅延後に出力する。
                    const double mix=std::clamp((n-static_cast<int>(processor->getLatencySamples()))/255.0,0.0,1.0);
                    for(int ch=0;ch<2;++ch) assert(std::abs(ramp[n][ch]-(dry[n][ch]*(1-mix)+wet[n][ch]*mix))<1e-6);
                }
            }
        }
    }
}
