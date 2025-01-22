use std::collections::HashMap;
use std::collections::HashSet;
use std::error::Error;
use std::fs;
mod app;
mod file;
pub use app::*;

pub fn load() -> Result<app::Config, Box<dyn Error>> {
    validate(parse()?)
}

fn match_predictor(predictor: file::Predictor) -> app::Predictor {
    match predictor {
        file::Predictor::Adaptive => app::Predictor::Adaptive,
        file::Predictor::Manual { thresholds } => app::Predictor::Manual {
            thresholds: thresholds
                .into_iter()
                .map(|(k, v)| {
                    (
                        k,
                        v.into_iter()
                            .map(|(k, v)| (k.parse::<u8>().unwrap(), v))
                            .collect(),
                    )
                })
                .collect(),
        },
    }
}

fn match_capturer(capturer: file::Capturer) -> app::Capturer {
    match capturer {
        file::Capturer::None => app::Capturer::None,
        file::Capturer::Wlroots => {
            log::warn!(
                "Config value capturer=\"wlroots\" is deprecated, use capturer=\"wayland\" instead"
            );
            app::Capturer::Wayland(app::WaylandProtocol::Any)
        }
        file::Capturer::Wayland => app::Capturer::Wayland(app::WaylandProtocol::Any),
        file::Capturer::ExtImageCopyCaptureV1 => {
            app::Capturer::Wayland(app::WaylandProtocol::ExtImageCopyCaptureV1)
        }
        file::Capturer::WlrScreencopyUnstableV1 => {
            app::Capturer::Wayland(app::WaylandProtocol::WlrScreencopyUnstableV1)
        }
        file::Capturer::WlrExportDmabufUnstableV1 => {
            app::Capturer::Wayland(app::WaylandProtocol::WlrExportDmabufUnstableV1)
        }
    }
}

fn parse() -> Result<app::Config, toml::de::Error> {
    let file_config = xdg::BaseDirectories::with_prefix("wluma")
        .ok()
        .and_then(|xdg| xdg.find_config_file("config.toml"))
        .and_then(|cfg_path| fs::read_to_string(cfg_path).ok())
        .unwrap_or_else(|| include_str!("../../config.toml").to_string());

    let parse_als_thresholds = |t: HashMap<String, String>| -> HashMap<u64, String> {
        t.into_iter()
            .map(|(k, v)| (k.parse().unwrap(), v))
            .collect()
    };

    toml::from_str(&file_config).map(|file_config: file::Config| app::Config {
        output: file_config
            .output
            .backlight
            .into_iter()
            .map(|o| {
                app::Output::Backlight(app::BacklightOutput {
                    name: o.name,
                    path: o.path,
                    min_brightness: 1,
                    capturer: match_capturer(o.capturer.unwrap_or_default()),
                    predictor: match_predictor(o.predictor.unwrap_or_default()),
                })
            })
            .chain(file_config.output.ddcutil.into_iter().map(|o| {
                app::Output::DdcUtil(app::DdcUtilOutput {
                    name: o.name,
                    min_brightness: 1,
                    capturer: match_capturer(o.capturer.unwrap_or_default()),
                    predictor: match_predictor(o.predictor.unwrap_or_default()),
                })
            }))
            .chain(file_config.keyboard.into_iter().map(|k| {
                app::Output::Backlight(app::BacklightOutput {
                    name: k.name,
                    path: k.path,
                    min_brightness: 0,
                    capturer: Capturer::None,
                    predictor: app::Predictor::Adaptive,
                })
            }))
            .collect(),

        als: match file_config.als {
            file::Als::Iio { path, thresholds } => app::Als::Iio {
                path,
                thresholds: parse_als_thresholds(thresholds),
            },
            file::Als::Webcam {
                video,
                sleep_ms,
                thresholds,
            } => app::Als::Webcam {
                video,
                sleep_ms,
                thresholds: parse_als_thresholds(thresholds),
            },
            file::Als::Time { thresholds } => app::Als::Time {
                thresholds: parse_als_thresholds(thresholds),
            },
            file::Als::None => app::Als::None,
        },
    })
}

fn validate(config: app::Config) -> Result<app::Config, Box<dyn Error>> {
    let names = config
        .output
        .iter()
        .map(|output| match output {
            app::Output::Backlight(app::BacklightOutput { name, .. }) => name,
            app::Output::DdcUtil(DdcUtilOutput { name, .. }) => name,
        })
        .collect::<HashSet<_>>();

    match (names.len(), names.len() == config.output.len()) {
        (0, _) => Err("No output or keyboard configured".into()),
        (_, false) => Err("Names of all outputs and keyboards are not unique".into()),
        _ => Ok(config),
    }
}
