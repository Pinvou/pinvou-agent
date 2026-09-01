use std::collections::{HashMap, HashSet};

use benchmark_core::{CompletedRun, OfficialScoreReport, PrivatePredictionContentType, TaskStatus};

use crate::{GAIA_LEVEL, GAIA_SPLIT, GaiaDataset};

pub const GAIA_SCORER_RUNTIME_PROFILE: &str = "hf-spaces-python-3.10-unicode-13.0";

const ASCII_PUNCTUATION: &str = "!\"#$%&'()*+,-./:;<=>?@[\\]^_`{|}~";
// `gaia-final/v1` is the task output contract. Core persists the resolved
// payload under its concrete content type, which is the run-bound scorer tag.
const GAIA_DURABLE_PREDICTION_TYPE: &str = "utf8-text/v1";
// Zero code points for every Unicode 13.0 `Nd` block used by the pinned
// Hugging Face Spaces default Python 3.10 runtime profile. Each block contains
// exactly ten contiguous decimal digits.
const PYTHON_DECIMAL_ZEROES: &[u32] = &[
    0x30, 0x660, 0x6F0, 0x7C0, 0x966, 0x9E6, 0xA66, 0xAE6, 0xB66, 0xBE6, 0xC66, 0xCE6, 0xD66,
    0xDE6, 0xE50, 0xED0, 0xF20, 0x1040, 0x1090, 0x17E0, 0x1810, 0x1946, 0x19D0, 0x1A80, 0x1A90,
    0x1B50, 0x1BB0, 0x1C40, 0x1C50, 0xA620, 0xA8D0, 0xA900, 0xA9D0, 0xA9F0, 0xAA50, 0xABF0, 0xFF10,
    0x104A0, 0x10D30, 0x11066, 0x110F0, 0x11136, 0x111D0, 0x112F0, 0x11450, 0x114D0, 0x11650,
    0x116C0, 0x11730, 0x118E0, 0x11950, 0x11C50, 0x11D50, 0x11DA0, 0x16A60, 0x16B50, 0x1D7CE,
    0x1D7D8, 0x1D7E2, 0x1D7EC, 0x1D7F6, 0x1E140, 0x1E2F0, 0x1E950, 0x1FBF0,
];

/// Deterministic port of the scorer pinned by `GAIA_SCORER_REVISION`.
pub fn question_scorer(model_answer: Option<&str>, ground_truth: &str) -> bool {
    let model_answer = model_answer.unwrap_or("None");

    if let Some(ground_truth_number) = parse_python_float(ground_truth) {
        return normalize_number(model_answer) == ground_truth_number;
    }

    if ground_truth.contains([',', ';']) {
        let ground_truth_elements = split_list(ground_truth);
        let model_answer_elements = split_list(model_answer);
        if ground_truth_elements.len() != model_answer_elements.len() {
            return false;
        }
        return model_answer_elements
            .into_iter()
            .zip(ground_truth_elements)
            .all(|(candidate, reference)| {
                if let Some(reference_number) = parse_python_float(reference) {
                    normalize_number(candidate) == reference_number
                } else {
                    normalize_string(candidate, false) == normalize_string(reference, false)
                }
            });
    }

    normalize_string(model_answer, true) == normalize_string(ground_truth, true)
}

pub(crate) fn score_dataset(dataset: &GaiaDataset, run: &CompletedRun) -> OfficialScoreReport {
    let partial = |evaluated, correct| {
        OfficialScoreReport::partial(evaluated, correct, GAIA_SPLIT, &GAIA_LEVEL.to_string())
    };
    if dataset.rows().is_empty() || run.outcomes().len() != dataset.rows().len() {
        return partial(0, 0);
    }

    let references = dataset
        .rows()
        .iter()
        .map(|row| (row.task_id(), row.reference()))
        .collect::<HashMap<_, _>>();
    let mut seen = HashSet::with_capacity(run.outcomes().len());
    let mut evaluated = 0_u64;
    let mut correct = 0_u64;
    for outcome in run.outcomes() {
        let Some(reference) = references.get(outcome.task_id()) else {
            return partial(0, 0);
        };
        if !seen.insert(outcome.task_id()) {
            return partial(0, 0);
        }
        match outcome.status() {
            TaskStatus::Failed | TaskStatus::Timeout | TaskStatus::Cancelled => {
                evaluated += 1;
                continue;
            }
            TaskStatus::Planned | TaskStatus::Running => return partial(0, 0),
            TaskStatus::Completed => {}
        }
        let Some(prediction) = outcome.prediction() else {
            return partial(0, 0);
        };
        if prediction.type_tag() != GAIA_DURABLE_PREDICTION_TYPE {
            return partial(0, 0);
        }
        let Ok(payload) = run.resolve_private_prediction(outcome) else {
            return partial(0, 0);
        };
        if payload.content_type() != PrivatePredictionContentType::Utf8TextV1 {
            return partial(0, 0);
        }
        let Ok(candidate) = std::str::from_utf8(payload.expose_to_scorer()) else {
            return partial(0, 0);
        };
        evaluated += 1;
        if question_scorer(Some(candidate), reference.expose_to_backend()) {
            correct += 1;
        }
    }

    if evaluated == dataset.rows().len() as u64 {
        OfficialScoreReport::compatible(evaluated, correct, GAIA_SPLIT, &GAIA_LEVEL.to_string())
    } else {
        partial(evaluated, correct)
    }
}

fn split_list(value: &str) -> Vec<&str> {
    value.split([',', ';']).collect()
}

fn normalize_number(value: &str) -> f64 {
    let cleaned = value.replace(['$', '%', ','], "");
    parse_python_float(&cleaned).unwrap_or(f64::INFINITY)
}

fn parse_python_float(value: &str) -> Option<f64> {
    let trimmed = value.trim();
    let decimal_normalized = normalize_python_decimal_digits(trimmed);
    let normalized = match decimal_normalized.to_ascii_lowercase().as_str() {
        "infinity" | "+infinity" => "inf".to_owned(),
        "-infinity" => "-inf".to_owned(),
        _ if valid_numeric_underscores(&decimal_normalized) => decimal_normalized.replace('_', ""),
        _ => decimal_normalized,
    };
    normalized.parse::<f64>().ok()
}

fn normalize_python_decimal_digits(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            let code_point = character as u32;
            let insertion = PYTHON_DECIMAL_ZEROES.partition_point(|zero| *zero <= code_point);
            insertion
                .checked_sub(1)
                .and_then(|index| code_point.checked_sub(PYTHON_DECIMAL_ZEROES[index]))
                .filter(|digit| *digit < 10)
                .and_then(|digit| char::from_digit(digit, 10))
                .unwrap_or(character)
        })
        .collect()
}

fn valid_numeric_underscores(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.iter().enumerate().all(|(index, byte)| {
        *byte != b'_'
            || (index > 0
                && index + 1 < bytes.len()
                && bytes[index - 1].is_ascii_digit()
                && bytes[index + 1].is_ascii_digit())
    })
}

fn normalize_string(value: &str, remove_punctuation: bool) -> String {
    let without_whitespace = value
        .chars()
        .filter(|character| !is_python_regex_whitespace(*character))
        .collect::<String>();
    let mut normalized = without_whitespace.to_lowercase();
    if remove_punctuation {
        normalized.retain(|character| !ASCII_PUNCTUATION.contains(character));
    }
    normalized
}

fn is_python_regex_whitespace(character: char) -> bool {
    // Python's Unicode `re` adds the four information separators to the
    // Unicode White_Space property otherwise represented by Rust.
    character.is_whitespace() || matches!(character, '\u{001c}'..='\u{001f}')
}
