//! System OCR backed by Tesseract through `leptess`.
//!
//! This mirrors the Windows OCR module: callers provide a raw RGBA image and
//! receive shared [`TextRecognition`] records whose bounds are in image-pixel
//! coordinates. Capture-region cropping and coordinate projection belong to the
//! `vision` module.

use std::fmt;

use auv_driver_common::geometry::Rect;
use auv_driver_common::vision::{OcrMatch, OcrMatches, RecognizedText, TextRecognition, TextRecognitionOptions};
#[cfg(target_os = "linux")]
use image::ImageEncoder;

#[derive(Debug)]
pub enum OcrError {
  Unsupported,
  InvalidImage { expected: usize, actual: usize },
  ImageTooLarge,
  Runtime(String),
}

impl fmt::Display for OcrError {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    match self {
      Self::Unsupported => write!(f, "linux OCR is unsupported on this target"),
      Self::InvalidImage { expected, actual } => {
        write!(f, "image buffer length {actual} did not match expected {expected} (width*height*4)")
      }
      Self::ImageTooLarge => write!(f, "image dimensions exceed the supported range"),
      Self::Runtime(message) => write!(f, "linux OCR runtime error: {message}"),
    }
  }
}

impl std::error::Error for OcrError {}

#[derive(Clone, Debug, PartialEq)]
struct Word {
  text: String,
  left: f64,
  top: f64,
  width: f64,
  height: f64,
  confidence: Option<f32>,
}

#[derive(Clone, Debug, Default, PartialEq)]
struct Line {
  text: String,
  words: Vec<Word>,
}

/// Recognizes text in a raw RGBA image using Tesseract.
///
/// The returned [`TextRecognition`] has one region per recognized text line,
/// with bounds in image-pixel coordinates. `recognition_languages` are mapped
/// to Tesseract language ids and joined with `+`; the default is `eng`.
#[cfg(target_os = "linux")]
pub fn recognize_text_in_rgba(rgba: &[u8], width: u32, height: u32, options: &TextRecognitionOptions) -> Result<TextRecognition, OcrError> {
  let (tsv, scale) = tesseract_tsv_from_rgba(rgba, width, height, options)?;
  Ok(text_recognition_from_tsv(&tsv, scale))
}

#[cfg(target_os = "linux")]
pub(crate) fn find_text_in_rgba(
  rgba: &[u8],
  width: u32,
  height: u32,
  query: &str,
  options: &TextRecognitionOptions,
) -> Result<OcrMatches, OcrError> {
  let (tsv, scale) = tesseract_tsv_from_rgba(rgba, width, height, options)?;
  Ok(text_matches_from_tsv(&tsv, query, scale))
}

/// Images whose longer side is at most this many pixels are upscaled 2x
/// before recognition; larger (HiDPI) captures already carry enough pixels.
#[cfg(target_os = "linux")]
const UPSCALE_MAX_SIDE: u32 = 2560;

/// Runs Tesseract and returns its TSV plus the factor that maps TSV pixel
/// coordinates back onto the input image.
#[cfg(target_os = "linux")]
fn tesseract_tsv_from_rgba(rgba: &[u8], width: u32, height: u32, options: &TextRecognitionOptions) -> Result<(String, f64), OcrError> {
  let expected = (width as usize) * (height as usize) * 4;
  if rgba.len() != expected {
    return Err(OcrError::InvalidImage {
      expected,
      actual: rgba.len(),
    });
  }
  if i32::try_from(width).is_err() || i32::try_from(height).is_err() {
    return Err(OcrError::ImageTooLarge);
  }

  // NOTICE: Tesseract misses small UI text at 1x. On 118 OSWorld captures
  // (1920x1080, scale 1) a 2x Lanczos upscale raised query matches from 6 to
  // 13 and lost none; a DPI hint alone recovered nothing. Bounds are scaled
  // back below, so callers still receive input-image pixel coordinates.
  let upscale = width.max(height) <= UPSCALE_MAX_SIDE;
  let mut png = Vec::new();
  let encoder = image::codecs::png::PngEncoder::new(&mut png);
  if upscale {
    let source = image::RgbaImage::from_raw(width, height, rgba.to_vec()).ok_or(OcrError::InvalidImage {
      expected,
      actual: rgba.len(),
    })?;
    let enlarged = image::imageops::resize(&source, width * 2, height * 2, image::imageops::FilterType::Lanczos3);
    encoder.write_image(enlarged.as_raw(), width * 2, height * 2, image::ExtendedColorType::Rgba8)
  } else {
    encoder.write_image(rgba, width, height, image::ExtendedColorType::Rgba8)
  }
  .map_err(|error| OcrError::Runtime(format!("failed to encode RGBA image as PNG: {error}")))?;

  let language = tesseract_language(options);
  // TODO(linux-tesseract-session-reuse): cache language-keyed engines only
  // after the Driver owns their thread/concurrency lifecycle; current profiling
  // shows image scope and remote frame transport dominate engine initialization.
  let mut tess =
    leptess::LepTess::new(None, &language).map_err(|error| OcrError::Runtime(format!("failed to initialize Tesseract: {error}")))?;

  // NOTICE(linux-tesseract-custom-words): leptess exposes Tesseract variables
  // but not a stable cross-version user-word injection surface. Custom word
  // weighting is deferred until an owner-approved OCR-quality slice needs it.
  tess.set_image_from_mem(&png).map_err(|error| OcrError::Runtime(format!("failed to load image into Tesseract: {error}")))?;
  tess.set_source_resolution(144);
  let tsv = tess.get_tsv_text(0).map_err(|error| OcrError::Runtime(format!("failed to read Tesseract TSV: {error}")))?;
  Ok((tsv, if upscale { 0.5 } else { 1.0 }))
}

#[cfg(not(target_os = "linux"))]
pub fn recognize_text_in_rgba(
  _rgba: &[u8],
  _width: u32,
  _height: u32,
  _options: &TextRecognitionOptions,
) -> Result<TextRecognition, OcrError> {
  Err(OcrError::Unsupported)
}

#[cfg(not(target_os = "linux"))]
pub(crate) fn find_text_in_rgba(
  _rgba: &[u8],
  _width: u32,
  _height: u32,
  _query: &str,
  _options: &TextRecognitionOptions,
) -> Result<OcrMatches, OcrError> {
  Err(OcrError::Unsupported)
}

#[cfg(target_os = "linux")]
fn tesseract_language(options: &TextRecognitionOptions) -> String {
  options
    .recognition_languages
    .as_ref()
    .map(|languages| languages.iter().map(|language| tesseract_language_tag(language)).collect::<Vec<_>>().join("+"))
    .filter(|language| !language.is_empty())
    .unwrap_or_else(|| "eng".to_string())
}

#[cfg(target_os = "linux")]
fn tesseract_language_tag(language: &str) -> String {
  match language {
    "en" | "en-US" | "en_US" | "eng" => "eng".to_string(),
    "zh" | "zh-CN" | "zh-Hans" | "zh_CN" | "chi_sim" => "chi_sim".to_string(),
    "zh-TW" | "zh-Hant" | "zh_TW" | "chi_tra" => "chi_tra".to_string(),
    other => other.replace('-', "_"),
  }
}

fn text_recognition_from_tsv(tsv: &str, scale: f64) -> TextRecognition {
  let lines = parse_tsv_lines(tsv, scale);
  let regions = lines
    .into_iter()
    .filter_map(|line| {
      let bounds = union_words(&line.words)?;
      Some(RecognizedText {
        text: line.text,
        bounds,
        confidence: mean_confidence(&line.words),
      })
    })
    .collect::<Vec<_>>();
  TextRecognition {
    origin: None,
    text: regions.iter().map(|region| region.text.as_str()).collect::<Vec<_>>().join("\n"),
    regions,
  }
}

fn text_matches_from_tsv(tsv: &str, query: &str, scale: f64) -> OcrMatches {
  let normalized_query = normalize_match_text(query);
  if normalized_query.is_empty() {
    return OcrMatches::default();
  }

  let mut matches = Vec::new();
  for line in parse_tsv_lines(tsv, scale) {
    for start in 0..line.words.len() {
      let mut candidate = String::new();
      for end in start..line.words.len() {
        if !candidate.is_empty() {
          candidate.push(' ');
        }
        candidate.push_str(&line.words[end].text);
        if !normalize_match_text(&candidate).contains(&normalized_query) {
          continue;
        }
        let words = &line.words[start..=end];
        let Some(bounds) = union_words(words) else {
          break;
        };
        matches.push(OcrMatch {
          text: candidate,
          confidence: mean_confidence(words).unwrap_or_default() as f64,
          bounds,
        });
        break;
      }
    }
  }
  let candidates = matches.clone();
  matches.retain(|candidate| !candidates.iter().any(|other| candidate != other && rect_strictly_contains(candidate.bounds, other.bounds)));
  OcrMatches { matches }
}

fn rect_strictly_contains(container: Rect, candidate: Rect) -> bool {
  let contains = candidate.origin.x >= container.origin.x
    && candidate.origin.y >= container.origin.y
    && candidate.origin.x + candidate.size.width <= container.origin.x + container.size.width
    && candidate.origin.y + candidate.size.height <= container.origin.y + container.size.height;
  contains && candidate.size.width * candidate.size.height < container.size.width * container.size.height
}

/// Normalizes query and recognized text the same way before matching.
///
/// Besides whitespace and case, this folds typographic quotes and dashes and
/// drops spaces next to `.` and `,`: Tesseract reads `Huashan.jpg` as
/// `Huashan. jpg` and `'youtube'` as `‘youtube’`, which missed exact queries.
fn normalize_match_text(text: &str) -> String {
  let folded = text
    .chars()
    .map(|character| match character {
      '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{2032}' => '\'',
      '\u{201C}' | '\u{201D}' | '\u{201E}' | '\u{2033}' => '"',
      '\u{2010}'..='\u{2015}' | '\u{2212}' => '-',
      other => other,
    })
    .collect::<String>();
  let collapsed = folded.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase();
  collapsed.replace(" .", ".").replace(". ", ".").replace(" ,", ",").replace(", ", ",")
}

/// Groups TSV word rows into lines; `scale` maps TSV pixels onto the input image.
fn parse_tsv_lines(tsv: &str, scale: f64) -> Vec<Line> {
  let mut lines = Vec::<((String, String, String), Line)>::new();
  for row in tsv.lines().skip(1) {
    let columns = row.split('\t').collect::<Vec<_>>();
    if columns.len() < 12 || columns[0] != "5" {
      continue;
    }
    let text = columns[11..].join("\t").trim().to_string();
    if text.is_empty() {
      continue;
    }
    let Some(word) = parse_word(&columns, text.clone(), scale) else {
      continue;
    };
    let key = (columns[2].to_string(), columns[3].to_string(), columns[4].to_string());
    if let Some((_, line)) = lines.iter_mut().find(|(existing, _)| *existing == key) {
      if !line.text.is_empty() {
        line.text.push(' ');
      }
      line.text.push_str(&text);
      line.words.push(word);
    } else {
      lines.push((
        key,
        Line {
          text,
          words: vec![word],
        },
      ));
    }
  }
  lines.into_iter().map(|(_, line)| line).collect()
}

fn parse_word(columns: &[&str], text: String, scale: f64) -> Option<Word> {
  let pixel = |index: usize| columns.get(index)?.parse::<f64>().ok().map(|value| value * scale);
  Some(Word {
    text,
    left: pixel(6)?,
    top: pixel(7)?,
    width: pixel(8)?,
    height: pixel(9)?,
    confidence: parse_confidence(columns.get(10)?),
  })
}

fn parse_confidence(raw: &str) -> Option<f32> {
  let confidence = raw.parse::<f32>().ok()?;
  if confidence < 0.0 {
    None
  } else {
    Some((confidence / 100.0).clamp(0.0, 1.0))
  }
}

fn union_words(words: &[Word]) -> Option<Rect> {
  let mut iter = words.iter();
  let first = iter.next()?;
  let mut min_x = first.left;
  let mut min_y = first.top;
  let mut max_x = first.left + first.width;
  let mut max_y = first.top + first.height;
  for word in iter {
    min_x = min_x.min(word.left);
    min_y = min_y.min(word.top);
    max_x = max_x.max(word.left + word.width);
    max_y = max_y.max(word.top + word.height);
  }
  Some(Rect::new(min_x, min_y, max_x - min_x, max_y - min_y))
}

fn mean_confidence(words: &[Word]) -> Option<f32> {
  let mut total = 0.0f32;
  let mut count = 0usize;
  for confidence in words.iter().filter_map(|word| word.confidence) {
    total += confidence;
    count += 1;
  }
  (count > 0).then_some(total / count as f32)
}

#[cfg(test)]
#[path = "ocr_test.rs"]
mod tests;
