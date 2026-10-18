use super::*;
use std::mem::{align_of, offset_of, size_of};

#[test]
fn linux_x86_64_layout() {
    // SDKヘッダ側の同じ値もtests/abi-smoke.cppで独立して検査する。
    assert_eq!(size_of::<FactoryInfo>(), 452);
    assert_eq!(size_of::<ClassInfo>(), 116);
    assert_eq!(size_of::<ClassInfo2>(), 440);
    assert_eq!(size_of::<BusInfo>(), 276);
    assert_eq!(size_of::<RoutingInfo>(), 12);
    assert_eq!(size_of::<ProcessSetup>(), 24);
    assert_eq!(offset_of!(ProcessSetup, sample_rate), 16);
    assert_eq!(size_of::<AudioBusBuffers>(), 24);
    assert_eq!(offset_of!(AudioBusBuffers, silence_flags), 8);
    assert_eq!(offset_of!(AudioBusBuffers, channel_buffers), 16);
    assert_eq!(size_of::<ProcessData>(), 80);
    assert_eq!(align_of::<ProcessData>(), 8);
    assert_eq!(offset_of!(ProcessData, inputs), 24);
    assert_eq!(offset_of!(ProcessData, process_context), 72);
    assert_eq!(size_of::<FactoryVTable>(), 8 * 8);
    assert_eq!(size_of::<ComponentVTable>(), 14 * 8);
    assert_eq!(size_of::<ProcessorVTable>(), 11 * 8);
}

#[test]
fn rejects_unknown_identifiers_and_clears_output() {
    let factory = GetPluginFactory();
    // SAFETY: 公開関数で取得した有効なFactoryと、型の一致する出力領域を渡す。
    unsafe {
        let table = &**factory.cast::<*const FactoryVTable>();
        let mut out = factory;
        assert_eq!(
            (table.create_instance)(factory, &[0; 16], &COMPONENT_IID, &mut out),
            NO_INTERFACE
        );
        assert!(out.is_null());
        out = factory;
        assert_eq!(
            (table.create_instance)(factory, &COMPONENT_CID, &[0; 16], &mut out),
            NO_INTERFACE
        );
        assert!(out.is_null());
        assert_eq!(
            (table.create_instance)(factory, &COMPONENT_CID, &COMPONENT_IID, ptr::null_mut()),
            INVALID_ARGUMENT
        );
        assert_eq!((table.unknown.release)(factory), 0);
    }
}

#[test]
fn interfaces_share_identity_and_own_the_instance() {
    let factory = GetPluginFactory();
    // SAFETY: 各queryが返す所有参照を保持し、対応するreleaseで一度だけ解放する。
    unsafe {
        let factory_table = &**factory.cast::<*const FactoryVTable>();
        let mut component = ptr::null_mut();
        assert_eq!(
            (factory_table.create_instance)(
                factory,
                &COMPONENT_CID,
                &COMPONENT_IID,
                &mut component
            ),
            OK
        );
        let component_table = &**component.cast::<*const ComponentVTable>();
        let mut processor = ptr::null_mut();
        assert_eq!(
            (component_table.unknown.query_interface)(component, &PROCESSOR_IID, &mut processor),
            OK
        );
        let processor_table = &**processor.cast::<*const ProcessorVTable>();
        let mut identity = ptr::null_mut();
        assert_eq!(
            (processor_table.unknown.query_interface)(processor, &UNKNOWN_IID, &mut identity),
            OK
        );
        assert_eq!(identity, component);
        assert_eq!((component_table.unknown.release)(identity), 2);
        assert_eq!((component_table.unknown.release)(component), 1);
        // ファクトリーとComponentの参照を解放してもProcessorの所有参照は有効。
        assert_eq!((factory_table.unknown.release)(factory), 0);
        assert_eq!((processor_table.get_latency_samples)(processor), 0);
        assert_eq!((processor_table.unknown.release)(processor), 0);
    }
}
