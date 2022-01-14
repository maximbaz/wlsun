use itertools::Itertools;
use std::sync::mpsc;
use std::thread;

mod als;
mod brightness;
mod config;
mod device_file;
mod frame;
mod predictor;

fn main() {
    let panic_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        panic_hook(panic_info);
        std::process::exit(1);
    }));

    env_logger::init();

    let config = match config::Config::load() {
        Ok(config) => config,
        Err(err) => panic!("Unable to load config: {}", err),
    };

    log::debug!("Using config: {:?}", config);

    let config_outputs = config.output;
    let config_als = config.als;

    let (als_txs, threads): (_, Vec<_>) = config_outputs
        .into_iter()
        .map(move |output| {
            let config = match config::Config::load() {
                Ok(config) => config,
                Err(err) => panic!("Unable to load config: {}", err),
            };

            let (als_tx, als_rx) = mpsc::channel();
            let (user_tx, user_rx) = mpsc::channel();
            let (prediction_tx, prediction_rx) = mpsc::channel();

            let capturer_config = output.clone();
            let output_name = match capturer_config {
                config::Output::Backlight(ref cfg) => &cfg.name,
                config::Output::DdcUtil(ref cfg) => &cfg.name,
            };

            (
                als_tx,
                vec![
                    std::thread::Builder::new()
                        .name(format!("backlight-{}", output_name))
                        .spawn(move || {
                            let brightness: Box<dyn brightness::Brightness> = match output {
                                config::Output::Backlight(cfg) => Box::new(
                                    brightness::Backlight::new(&cfg.path)
                                        .expect("Unable to initialize output backlight"),
                                ),
                                config::Output::DdcUtil(cfg) => Box::new(
                                    brightness::DdcUtil::new(&cfg.name)
                                        .expect("Unable to initialize output ddcutil"),
                                ),
                            };

                            let mut brightness_controller =
                                brightness::Controller::new(brightness, user_tx, prediction_rx);

                            brightness_controller.run();
                        })
                        .expect("Unable to start als thread"),
                    std::thread::Builder::new()
                        .name(format!("predictor-{}", output_name))
                        .spawn(move || {
                            let frame_processor: Box<dyn frame::processor::Processor> =
                                match config.frame.processor {
                                    config::Processor::Vulkan => Box::new(
                                        frame::processor::vulkan::Processor::new()
                                            .expect("Unable to initialize Vulkan"),
                                    ),
                                };

                            let frame_capturer: Box<dyn frame::capturer::Capturer> =
                                match config.frame.capturer {
                                    config::Capturer::Wlroots => Box::new(
                                        frame::capturer::wlroots::Capturer::new(frame_processor),
                                    ),
                                    config::Capturer::None => {
                                        Box::new(frame::capturer::none::Capturer::default())
                                    }
                                };

                            let output_name = match capturer_config {
                                config::Output::Backlight(cfg) => cfg.name,
                                config::Output::DdcUtil(cfg) => cfg.name,
                            };

                            let controller = predictor::Controller::new(
                                prediction_tx,
                                user_rx,
                                als_rx,
                                true,
                                &output_name,
                            );
                            frame_capturer.run(&output_name, controller)
                        })
                        .expect("Unable to start predictor thread"),
                ],
            )
        })
        .unzip();

    let threads = threads
        .into_iter()
        .flatten()
        .chain(std::iter::once(
            thread::Builder::new()
                .name("als".to_string())
                .spawn(move || {
                    let als: Box<dyn als::Als> = match config_als {
                        config::Als::Iio {
                            path, thresholds, ..
                        } => Box::new(
                            als::iio::Als::new(&path, thresholds)
                                .expect("Unable to initialize ALS IIO sensor"),
                        ),
                        config::Als::Time { thresholds, .. } => {
                            Box::new(als::time::Als::new(thresholds))
                        }
                        config::Als::Webcam {
                            video, thresholds, ..
                        } => Box::new({
                            let (webcam_tx, webcam_rx) = mpsc::channel();
                            thread::Builder::new()
                                .name("als-webcam".to_string())
                                .spawn(move || {
                                    als::webcam::Webcam::new(webcam_tx, video).run();
                                })
                                .expect("Unable to start webcam als");
                            als::webcam::Als::new(webcam_rx, thresholds)
                        }),
                        config::Als::None => Box::new(als::none::Als::default()),
                    };

                    als::controller::Controller::new(als, als_txs).run();
                })
                .expect("Unable to start als"),
        ))
        .collect_vec();

    println!("Continue adjusting brightness and wluma will learn your preference over time.");

    threads
        .into_iter()
        .for_each(|t| t.join().expect("Error running thread"));
}
