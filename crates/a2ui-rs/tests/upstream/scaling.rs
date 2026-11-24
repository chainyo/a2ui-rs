use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

use a2ui_rs::catalog::Catalog;
use a2ui_rs::validation::{MessageProcessor, ValidationConfig};
use serde::Deserialize;
use serde_json::Value;

use super::TestError;

pub(super) const SUITE: &str = "core/message_processor_scaling.yaml";
const RUNS: usize = 5;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TimingCase {
    name: String,
    protocol_version: String,
    action: String,
    operation: String,
    catalog_paths: Vec<String>,
    args: TimingArgs,
    payload: Vec<Segment>,
    sizes: [usize; 2],
    #[serde(rename = "max_ratio")]
    max_ratio: f64,
}

#[derive(Deserialize)]
struct TimingArgs {
    validation_config: TimingConfig,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum TimingConfig {
    None,
    Relaxed,
}

impl TimingConfig {
    fn rules(&self) -> ValidationConfig {
        match self {
            Self::Relaxed => ValidationConfig::RELAXED,
            // The upstream no-config case permits streaming unresolved children.
            Self::None => ValidationConfig {
                allow_dangling_references: true,
                ..ValidationConfig::default()
            },
        }
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Segment {
    Text(String),
    Repeat {
        repeat: String,
        #[serde(default)]
        separator: String,
    },
}

pub(super) fn run(upstream: &Path) -> Result<(), TestError> {
    let cases: Vec<TimingCase> = serde_saphyr::from_str(&fs::read_to_string(
        upstream.join("conformance").join(SUITE),
    )?)?;
    if cases.is_empty() {
        return Err("missing processor scaling cases".into());
    }
    let common = read_json(upstream, "specification/v1_0/json/common_types.json")?;
    let mut failures = Vec::new();
    for case in cases {
        let (small, large) = measure(upstream, &case, &common)?;
        let growth = large.div_duration_f64(small);
        println!(
            "{}: {:?} -> {:?}, {:.2}x (limit {:.2}x)",
            case.name, small, large, growth, case.max_ratio
        );
        if growth > case.max_ratio {
            failures.push(case.name);
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "processor growth exceeded the pinned limit: {}",
            failures.join(", ")
        )
        .into())
    }
}

fn measure(
    upstream: &Path,
    case: &TimingCase,
    common: &Value,
) -> Result<(Duration, Duration), TestError> {
    if case.protocol_version != "1.0"
        || case.action != "measure_growth"
        || case.operation != "process_messages"
        || case.sizes[0] == 0
        || case.sizes[0].checked_mul(4) != Some(case.sizes[1])
        || !case.max_ratio.is_finite()
        || case.max_ratio <= 0.0
    {
        return Err(format!("invalid scaling fixture {}", case.name).into());
    }
    let documents = case
        .catalog_paths
        .iter()
        .map(|path| read_json(upstream, path))
        .collect::<Result<Vec<_>, _>>()?;
    let payloads = case.sizes.map(|size| expand(&case.payload, size));
    let payloads = [
        serde_json::from_str(&payloads[0])?,
        serde_json::from_str(&payloads[1])?,
    ];

    let sample = |index: usize| {
        run_once(
            &payloads[index],
            case.sizes[index],
            &documents,
            common,
            case.args.validation_config.rules(),
        )
    };
    sample(0)?;
    sample(1)?;
    let mut durations = [[Duration::ZERO; 2]; RUNS];
    for (run, pair) in durations.iter_mut().enumerate() {
        for index in [run % 2, 1 - run % 2] {
            pair[index] = sample(index)?;
        }
    }
    let [small, large] = [0, 1].map(|index| {
        let mut samples = durations.map(|pair| pair[index]);
        samples.sort_unstable();
        samples[RUNS / 2]
    });
    if small.is_zero() {
        return Err(format!("zero-duration sample for {}", case.name).into());
    }
    Ok((small, large))
}

fn run_once(
    payload: &Value,
    size: usize,
    documents: &[Value],
    common: &Value,
    config: ValidationConfig,
) -> Result<Duration, TestError> {
    let mut processor = MessageProcessor::new(config);
    for document in documents {
        processor.add_catalog(Catalog::from_json(document.clone(), common)?)?;
    }
    let timer = Instant::now();
    let result = processor.apply_json(payload);
    let elapsed = timer.elapsed();
    result?;
    let surface = processor.get("s1").ok_or("missing scaling surface")?;
    if surface.components().len() != size + 1 {
        return Err("processor did not apply all timing messages".into());
    }
    Ok(elapsed)
}

fn expand(template: &[Segment], size: usize) -> String {
    let mut result = String::new();
    for segment in template {
        match segment {
            Segment::Text(text) => result.push_str(text),
            Segment::Repeat { repeat, separator } => {
                for index in 0..size {
                    if index != 0 {
                        result.push_str(separator);
                    }
                    result.push_str(&repeat.replace("{i}", &index.to_string()));
                }
            }
        }
    }
    result
}

fn read_json(upstream: &Path, path: &str) -> Result<Value, TestError> {
    Ok(serde_json::from_slice(&fs::read(upstream.join(path))?)?)
}
