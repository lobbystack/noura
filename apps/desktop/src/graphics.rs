//! Windows graphics initialization, before any driver is loaded.

use eframe::{
    egui_wgpu::{WgpuConfiguration, WgpuSetup, WgpuSetupCreateNew},
    wgpu::Backends,
};

pub(crate) fn configuration(requested: Option<Backends>) -> WgpuConfiguration {
    let mut setup = WgpuSetupCreateNew::without_display_handle();
    // Some Intel Vulkan drivers crash with an access violation during
    // initialization, before wgpu can return an error or try another adapter.
    // Exclude Vulkan at instance creation, not just during adapter selection.
    // Keep WGPU_BACKEND available as an explicit diagnostic override.
    setup.instance_descriptor.backends = requested.unwrap_or(Backends::DX12);
    WgpuConfiguration {
        wgpu_setup: WgpuSetup::CreateNew(setup),
        ..WgpuConfiguration::default().with_surface_config(eframe::SurfaceConfig::LOW_LATENCY)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_instance_excludes_the_crashing_vulkan_driver() {
        let WgpuSetup::CreateNew(setup) = configuration(None).wgpu_setup else {
            panic!("startup must configure a new instance");
        };
        assert_eq!(setup.instance_descriptor.backends, Backends::DX12);
        assert!(!setup.instance_descriptor.backends.contains(Backends::VULKAN));
    }

    #[test]
    fn explicit_backend_override_is_preserved() {
        let requested = Backends::VULKAN | Backends::GL;
        let WgpuSetup::CreateNew(setup) = configuration(Some(requested)).wgpu_setup else {
            panic!("startup must configure a new instance");
        };
        assert_eq!(setup.instance_descriptor.backends, requested);
    }
}
