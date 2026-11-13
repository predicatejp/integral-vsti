// Copyright (C) 2026 Yusuke Oda
// SPDX-License-Identifier: AGPL-3.0-only

// IUnitInfoの全メソッドとProgram変更を公式SDKのABIで検証する。
class ProgramHandler final : public IComponentHandler {
public:
    IEditController* controller = nullptr;
    uint32 refs = 1;
    int calls = 0;
    tresult PLUGIN_API queryInterface(const TUID, void** out) override { *out = nullptr; return kNoInterface; }
    uint32 PLUGIN_API addRef() override { return ++refs; }
    uint32 PLUGIN_API release() override { return --refs; }
    tresult PLUGIN_API beginEdit(ParamID) override { assert(false); return kResultFalse; }
    tresult PLUGIN_API performEdit(ParamID, ParamValue) override { assert(false); return kResultFalse; }
    tresult PLUGIN_API endEdit(ParamID) override { assert(false); return kResultFalse; }
    tresult PLUGIN_API restartComponent(int32 flags) override {
        assert(flags == kParamValuesChanged);
        assert(controller->getParamNormalized(waveformId) == 0.0);
        ++calls;
        // コールバック中のhandler交換でも所有参照が保持される。
        assert(controller->setComponentHandler(nullptr) == kResultOk);
        assert(refs == 2);
        return kResultOk;
    }
};

template <typename Sample>
void checkPrograms(IPluginFactory* factory, const TUID cid, int32 precision) {
    constexpr ParamID programId = 0x50524f47;
    void* object = nullptr;
    assert(factory->createInstance(cid, IComponent_iid, &object) == kResultOk);
    auto* component = static_cast<IComponent*>(object);
    auto* controller = query<IEditController>(component, IEditController_iid);
    auto* processor = query<IAudioProcessor>(component, IAudioProcessor_iid);
    auto* units = query<IUnitInfo>(controller, IUnitInfo_iid);
    auto* identity = query<FUnknown>(units, FUnknown_iid);
    assert(identity == component);
    identity->release();
    assert(component->initialize(factory) == kResultOk);
    assert(units->getUnitCount() == 1 && units->getProgramListCount() == 1);
    UnitInfo unit {};
    ProgramListInfo list {};
    assert(units->getUnitInfo(0, unit) == kResultOk);
    assert(unit.id == kRootUnitId && unit.parentUnitId == kNoParentUnitId && unit.programListId == 0);
    assert(units->getUnitInfo(1, unit) == kInvalidArgument);
    assert(units->getProgramListInfo(0, list) == kResultOk && list.id == 0 && list.programCount == 1);
    assert(units->getProgramListInfo(-1, list) == kInvalidArgument);
    String128 name {};
    assert(units->getProgramName(list.id, 0, name) == kResultOk && std::u16string(name) == u"Sine");
    assert(units->getProgramName(1, 0, name) == kInvalidArgument);
    assert(units->getProgramName(0, 1, name) == kInvalidArgument);
    assert(units->getProgramInfo(0, 0, "Instrument", name) == kResultFalse);
    assert(units->hasProgramPitchNames(0, 0) == kResultFalse);
    assert(units->getProgramPitchName(0, 0, 60, name) == kResultFalse);
    assert(units->getSelectedUnit() == 0 && units->selectUnit(0) == kResultOk);
    assert(units->selectUnit(1) == kInvalidArgument);
    UnitID id = -1;
    assert(units->getUnitByBus(kEvent, kInput, 0, 15, id) == kResultOk && id == 0);
    assert(units->getUnitByBus(kAudio, kOutput, 0, 1, id) == kResultOk && id == 0);
    assert(units->getUnitByBus(kEvent, kInput, 0, 16, id) == kResultFalse);
    assert(units->setUnitProgramData(0, 0, nullptr) == kNotImplemented);
    ParameterInfo info {};
    assert(controller->getParameterInfo(134, info) == kResultOk && info.id == programId);
    assert(info.unitId == unit.id && info.stepCount == list.programCount - 1);
    assert(info.flags == (ParameterInfo::kIsList | ParameterInfo::kIsProgramChange));
    assert(controller->getParamStringByValue(programId, 1.0, name) == kResultOk && std::u16string(name) == u"Sine");
    ParamValue normalized = -1;
    assert(controller->getParamValueByString(programId, name, normalized) == kResultOk && normalized == 0.0);
    assert(controller->normalizedParamToPlain(programId, 1.0) == 0.0);
    assert(controller->plainParamToNormalized(programId, 0.0) == 0.0);
    assert(controller->setParamNormalized(programId, std::nan("")) == kInvalidArgument);
    assert(controller->setParamNormalized(programId, -1) == kInvalidArgument);

    const std::array<double, 134> sine {0,0,0,0,1,0,1,0,1,0,4800,1,0,0,0,0,0,1,0,0,0,0,0,0,0,-std::numeric_limits<double>::infinity(),0,0,-std::numeric_limits<double>::infinity(),0,0,0,0,100,0.0,250.0,0.0,1000.0,0.0,1.0,4000.0,0.0,0,0,0,0,0,100,0,0,100,5,35,25,5,35,25,5,35,25,0,0,0,2,6000,100,25,2,6000,100,25,2,6000,100,25,20,2,0.7,0,0,50,20,2,0.7,0,0,50,20,2,0.7,0,0,50,1000,1200,.5,50,50,1000,1200,.5,50,50,1000,1200,.5,50,50,0,-18,4,10,100,6,0,100,-18,4,10,100,6,0,100,-18,4,10,100,6,0,100,100,0,0,0};
    ProgramHandler handler; handler.controller = controller;
    assert(controller->setComponentHandler(&handler) == kResultOk);
    // 全設定をSineと異なる値にしてから再選択し、追加済み項目も復元されることを確認する。
    for (int32 i = 0; i < static_cast<int32>(sine.size()); ++i) {
        assert(controller->getParameterInfo(i, info) == kResultOk);
        const double changed = controller->plainParamToNormalized(info.id, sine[i]) == 0.0 ? 1.0 : 0.0;
        assert(controller->setParamNormalized(info.id, changed) == kResultOk);
    }
    assert(controller->setParamNormalized(programId, 0.0) == kResultOk);
    assert(handler.calls == 1 && handler.refs == 1);
    for (int32 i = 0; i < 134; ++i) {
        assert(controller->getParameterInfo(i, info) == kResultOk);
        const auto plain = controller->normalizedParamToPlain(info.id, controller->getParamNormalized(info.id));
        assert(plain == sine[i] || std::abs(plain - sine[i]) < 1e-10);
    }
    // 停止中の0サンプルで選択し、実際の音色設定を保存する。
    Points program; program.id = programId; program.values = {{0, 0.0}};
    Changes changes; changes.queues = {&program};
    ProcessData data; data.symbolicSampleSize = precision; data.inputParameterChanges = &changes;
    assert(processor->process(data) == kResultOk);
    StateStream saved;
    assert(component->getState(&saved) == kResultOk);
    assert(controller->setComponentState(&saved) == kResultOk);
    for (int32 i = 0; i < 134; ++i) {
        assert(controller->getParameterInfo(i, info) == kResultOk);
        const auto plain = controller->normalizedParamToPlain(info.id, controller->getParamNormalized(info.id));
        assert(plain == sine[i] || std::abs(plain - sine[i]) < 1e-10);
    }

    ProcessSetup setup {kRealtime, precision, 33, 48000.0};
    assert(processor->setupProcessing(setup) == kResultOk);
    LatencyReference<Sample> reference(processor);
    assert(component->activateBus(kAudio, kOutput, 0, true) == kResultOk);
    assert(component->setActive(true) == kResultOk);
    assert(processor->setProcessing(true) == kResultOk);
    Points wave; wave.values = {{0, .5}, {9, .5}, {15, .5}};
    program.values = {{-1, 0.0}, {5, std::nan("")}, {5, 0.0}, {15, 0.0}, {23, 0.0}, {33, 0.0}};
    // 同位置はProgram→個別設定→ノートの順。番号0の再選択でもSineを読み直す。
    changes.queues = {&wave, &program};
    Events events; events.values = {note(true, 0, 1), note(true, 23, 2), note(false, 31, 2)};
    std::array<Sample, 33> left {}, right {};
    Sample* channels[] = {left.data(), right.data()};
    AudioBusBuffers bus; bus.numChannels = 2;
    if constexpr (std::is_same_v<Sample, float>) bus.channelBuffers32 = channels;
    else bus.channelBuffers64 = channels;
    data.numSamples = 33; data.numOutputs = 1; data.outputs = &bus; data.inputEvents = &events;
    assert(processor->process(data) == kResultOk);
    for (int i = 0; i < 33; ++i) {
        double expected = i < 31 ? bandlimited(2, i * 440.0 / 48000.0, 440.0 / 48000.0) : 0.0;
        if ((i >= 5 && i < 9) || (i >= 23 && i < 31))
            expected = .5 * std::sqrt(2.0) * std::sin(2 * std::acos(-1.0) * i * 440 / 48000.0);
        assert(std::abs(left[i] - reference.next(expected)) < 1e-6 && left[i] == right[i]);
    }
    // Programと個別変更の結果が現在のDSP設定と同値でも、保存値を一致させる。
    data.numSamples = 0; events.values.clear(); program.values.clear(); wave.values = {{0, .5}};
    assert(processor->process(data) == kResultOk);
    program.values = {{0, 0.0}};
    assert(processor->process(data) == kResultOk);
    StateStream unchanged; assert(component->getState(&unchanged) == kResultOk);
    assert(unchanged.bytes[8] == 2); // Square
    // Programの切替後はプリセット値から次のVolume点まで補間する。
    Points phase, volume;
    phase.id = 0x50484153; phase.values = {{0, .5}}; // Zero
    volume.id = 0x4d564f4c; volume.values = {{8, 480.0/720}}; // -12 dB
    wave.values = {{0, 0.0}}; program.values = {{4, 0.0}};
    changes.queues = {&program, &wave, &phase, &volume};
    data.numSamples = 9; events.values = {note(true, 0, 3)};
    assert(processor->process(data) == kResultOk);
    for (int i = 0; i < 9; ++i) {
        const auto db = i < 4 ? -12.0 * (i + 1) / 9 : -12.0 * (i - 4) / 4;
        const auto gain = std::pow(10.0, db / 20);
        const auto expected = .5 * std::sqrt(2.0) * std::sin(2 * std::acos(-1.0) * i * 440 / 48000.0);
        assert(std::abs(left[i] - reference.next(expected) * gain) < 1e-6 && left[i] == right[i]);
    }
    reference.scaleHistory(std::pow(10.0, -12.0 / 20));
    changes.queues = {&wave, &program};
    reference.flush();
    assert(processor->setProcessing(false) == kResultOk);
    assert(component->setActive(false) == kResultOk);
    data = ProcessData {}; data.symbolicSampleSize = precision; data.inputParameterChanges = &changes;
    // 編集値を保存・復元しても元のプリセットで上書きしない。再選択時には戻す。
    program.values.clear(); wave.values = {{0, .5}};
    assert(processor->process(data) == kResultOk);
    StateStream edited; assert(component->getState(&edited) == kResultOk);
    assert(component->setState(&edited) == kResultOk);
    assert(controller->getParamNormalized(waveformId) == .5);
    program.values = {{0, 0.0}}; wave.values.clear();
    assert(controller->setParamNormalized(programId, 0.0) == kResultOk);
    assert(processor->process(data) == kResultOk);
    StateStream reselected; assert(component->getState(&reselected) == kResultOk);
    assert(reselected.bytes == saved.bytes);
    assert(component->terminate() == kResultOk);
    units->release(); controller->release(); processor->release();
    assert(component->release() == 0);
}
