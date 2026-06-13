use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use pulldown_cmark::{Event, HeadingLevel, Options, Tag, TagEnd};
use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::helpers::llhttp;

#[path = "../tests/helpers/mod.rs"]
mod helpers;

const FIXTURE_PREFIX: &str = "tests/fixtures/llhttp";

#[derive(Debug)]
struct RawCase {
  titles: Vec<String>,
  source_path: String,
  source_line: usize,
  meta: Option<serde_json::Value>,
  http: Option<String>,
  log: Option<String>,
}

#[derive(Debug)]
struct CaseItem {
  path: String,
  fixture: FixtureForWrite,
}

#[derive(Debug, Serialize, Deserialize)]
struct FixtureForWrite {
  path: String,
  name: String,
  checked: bool,
  source: llhttp::Source,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  meta: Option<serde_json::Value>,
  input: Vec<String>,
  llhttp: Vec<String>,
  output: Option<Vec<llhttp::Event>>,
}

fn clean_heading(title: &str) -> String {
  title.replace('`', "").trim().to_string()
}

fn decode_html_entities(value: &str) -> String {
  let re = Regex::new(r"&(?:quot|apos|amp|lt|gt|#(\d+)|#x([0-9a-fA-F]+));").unwrap();
  re.replace_all(value, |caps: &regex::Captures| {
    let whole = caps.get(0).unwrap().as_str();
    match whole {
      "&quot;" => "\"".to_string(),
      "&apos;" => "'".to_string(),
      "&amp;" => "&".to_string(),
      "&lt;" => "<".to_string(),
      "&gt;" => ">".to_string(),
      _ => {
        if let Some(c) = caps
          .get(1)
          .and_then(|dec| dec.as_str().parse::<u32>().ok())
          .and_then(char::from_u32)
        {
          return c.to_string();
        }
        if let Some(c) = caps
          .get(2)
          .and_then(|hex| u32::from_str_radix(hex.as_str(), 16).ok())
          .and_then(char::from_u32)
        {
          return c.to_string();
        }
        whole.to_string()
      }
    }
  })
  .to_string()
}

fn parse_html_meta(value: &str) -> Option<serde_json::Value> {
  let re1 = Regex::new(r"(?s)<!--\s*meta=(.*?)\s*-->").unwrap();
  let re2 = Regex::new(r"(?s)^\s*meta=(.*?)\s*$").unwrap();

  let captured = re1
    .captures(value)
    .or_else(|| re2.captures(value))
    .and_then(|c| c.get(1))
    .map(|m| m.as_str().trim().to_string())?;

  let decoded = decode_html_entities(&captured);
  let meta: serde_json::Value = serde_json::from_str(&decoded).ok()?;

  match &meta {
    serde_json::Value::Object(obj) if !obj.is_empty() => Some(meta),
    _ => None,
  }
}

fn has_meta(value: &Option<serde_json::Value>) -> bool {
  match value {
    Some(serde_json::Value::Object(obj)) => !obj.is_empty(),
    _ => false,
  }
}

fn title_case(value: &str) -> String {
  value
    .split_whitespace()
    .filter(|s| !s.is_empty())
    .map(|part| {
      let mut chars = part.chars();
      match chars.next() {
        None => String::new(),
        Some(first) => {
          let upper: String = first.to_uppercase().collect();
          format!("{}{}", upper, chars.as_str())
        }
      }
    })
    .collect::<Vec<_>>()
    .join(" ")
}

fn slugify(part: &str) -> String {
  use unicode_normalization::UnicodeNormalization;

  let cleaned = clean_heading(part);
  let nfkd: String = cleaned.chars().nfkd().collect();
  let lowered = nfkd.to_lowercase();

  let non_alnum = Regex::new(r"[^a-z0-9\s\-]").unwrap();
  let replaced = non_alnum.replace_all(&lowered, " ").to_string();
  let trimmed = replaced.trim().to_string();

  let ws_under = Regex::new(r"[\s_]+").unwrap();
  let dashed = ws_under.replace_all(&trimmed, "-").to_string();

  let multi_dash = Regex::new(r"-+").unwrap();
  let single = multi_dash.replace_all(&dashed, "-").to_string();

  let edge_dash = Regex::new(r"^-+|-+$").unwrap();
  edge_dash.replace_all(&single, "").to_string()
}

fn stringify_fixture(fixture: &FixtureForWrite) -> String {
  let yaml_body = serde_yaml::to_string(fixture).unwrap();
  format!("---\n{}", yaml_body)
}

fn normalize_for_comparison(value: &serde_yaml::Value) -> serde_json::Value {
  match value {
    serde_yaml::Value::Sequence(seq) => {
      serde_json::Value::Array(seq.iter().map(normalize_for_comparison).collect())
    }
    serde_yaml::Value::Mapping(map) => {
      let mut sorted = BTreeMap::new();
      for (k, v) in map {
        let key = match k {
          serde_yaml::Value::String(s) => s.clone(),
          _ => serde_yaml::to_string(k).unwrap(),
        };
        if key == "checked" {
          continue;
        }
        sorted.insert(key, normalize_for_comparison(v));
      }
      serde_json::Value::Object(sorted.into_iter().collect())
    }
    serde_yaml::Value::Bool(b) => serde_json::Value::Bool(*b),
    serde_yaml::Value::Number(n) => {
      if let Some(i) = n.as_i64() {
        serde_json::Value::Number(i.into())
      } else if let Some(u) = n.as_u64() {
        serde_json::Value::Number(u.into())
      } else if let Some(f) = n.as_f64() {
        serde_json::Value::Number(serde_json::Number::from_f64(f).unwrap())
      } else {
        serde_json::Value::Null
      }
    }
    serde_yaml::Value::String(s) => serde_json::Value::String(s.clone()),
    serde_yaml::Value::Null => serde_json::Value::Null,
    serde_yaml::Value::Tagged(t) => normalize_for_comparison(&t.value),
  }
}

fn parse_markdown_cases(raw: &str, source_path: &str) -> Vec<RawCase> {
  let opts = Options::empty();

  let mut cases: Vec<RawCase> = Vec::new();
  let mut current_section: Option<String> = None;
  let mut current_case: Option<RawCase> = None;

  // Track line numbers from byte offsets
  let line_starts: Vec<usize> = std::iter::once(0)
    .chain(raw.match_indices('\n').map(|(i, _)| i + 1))
    .collect();

  let offset_to_line = |offset: usize| -> usize {
    match line_starts.binary_search(&offset) {
      Ok(i) => i + 1,
      Err(i) => i,
    }
  };

  let mut heading_text = String::new();
  let mut in_heading: Option<HeadingLevel> = None;
  let mut code_lang = String::new();
  let mut code_text = String::new();
  let mut in_code = false;
  let mut current_offset: usize = 0;

  for (event, range) in pulldown_cmark::Parser::new_ext(raw, opts).into_offset_iter() {
    match event {
      Event::Start(Tag::Heading { level, .. }) => {
        if level == HeadingLevel::H2 || level == HeadingLevel::H3 {
          // Flush previous case
          if let Some(case) = current_case.take().filter(|c| c.http.is_some() && c.log.is_some()) {
            cases.push(case);
          }

          in_heading = Some(level);
          heading_text.clear();
          current_offset = range.start;
        }
      }
      Event::Text(text) if in_heading.is_some() => {
        heading_text.push_str(&text);
      }
      Event::Code(code) if in_heading.is_some() => {
        heading_text.push_str(&code);
      }
      Event::End(TagEnd::Heading(_)) => {
        if let Some(level) = in_heading.take() {
          let cleaned = clean_heading(&heading_text);
          let line = offset_to_line(current_offset);

          if level == HeadingLevel::H2 {
            current_section = Some(cleaned.clone());
            current_case = Some(RawCase {
              titles: vec![cleaned],
              source_path: source_path.to_string(),
              source_line: line,
              meta: None,
              http: None,
              log: None,
            });
          } else if level == HeadingLevel::H3 {
            let titles = if let Some(ref section) = current_section {
              vec![section.clone(), cleaned.clone()]
            } else {
              vec![cleaned.clone()]
            };
            current_case = Some(RawCase {
              titles,
              source_path: source_path.to_string(),
              source_line: line,
              meta: None,
              http: None,
              log: None,
            });
          }
        }
      }
      Event::Start(Tag::CodeBlock(pulldown_cmark::CodeBlockKind::Fenced(lang))) => {
        code_lang = lang.to_lowercase().to_string();
        code_text.clear();
        in_code = true;
      }
      Event::Text(text) if in_code => {
        code_text.push_str(&text);
      }
      Event::End(TagEnd::CodeBlock) => {
        if in_code {
          in_code = false;
          if let Some(ref mut case) = current_case {
            // Remove trailing newline from code block content (pulldown-cmark includes it)
            let content = code_text.trim_end_matches('\n').to_string();
            if code_lang == "http" {
              case.http = Some(content);
            } else if code_lang == "log" {
              case.log = Some(content);
            }
          }
        }
      }
      Event::Html(html) | Event::InlineHtml(html) => {
        if let Some(ref mut case) = current_case.as_mut().filter(|c| c.http.is_none())
          && let Some(meta) = parse_html_meta(&html)
        {
          let existing = case.meta.take().unwrap_or(serde_json::Value::Object(Default::default()));
          if let (serde_json::Value::Object(mut map), serde_json::Value::Object(new_map)) = (existing, meta) {
            map.extend(new_map);
            case.meta = Some(serde_json::Value::Object(map));
          }
        }
      }
      _ => {}
    }
  }

  // Flush final case
  if let Some(case) = current_case.take().filter(|c| c.http.is_some() && c.log.is_some()) {
    cases.push(case);
  }

  cases
}

fn process_section(llhttp_root: &Path, _output_root: &Path, fixture_root: &Path, section: &str, seen_files: &mut HashSet<String>) {
  let source = llhttp_root.join("test").join(section);

  // Find markdown files
  let mut files: Vec<PathBuf> = Vec::new();
  collect_md_files(&source, &mut files);
  files.sort();

  let mut cases: Vec<CaseItem> = Vec::new();
  let mut used_names: HashSet<String> = HashSet::new();

  for file in &files {
    let raw = fs::read_to_string(file).unwrap();
    let source_path = file
      .strip_prefix(llhttp_root)
      .unwrap()
      .to_str()
      .unwrap()
      .replace('\\', "/");

    let parsed = parse_markdown_cases(&raw, &source_path);

    for item in parsed {
      if item.http.is_none() || item.log.is_none() {
        continue;
      }

      // Build deterministic fixture file name from titles
      let name: String = item
        .titles
        .iter()
        .map(|part| slugify(part))
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-");

      let base_name = if name.is_empty() { "test".to_string() } else { name.clone() };
      let mut file_name = format!("{}.yml", base_name);

      if !used_names.contains(&file_name) {
        used_names.insert(file_name.clone());
      } else {
        let mut counter = 2;
        loop {
          let candidate = format!("{}-{}.yml", base_name, counter);
          if !used_names.contains(&candidate) {
            file_name = candidate;
            break;
          }
          counter += 1;
        }
        used_names.insert(file_name.clone());
      }

      let input = item.http.unwrap();
      let log = item.log.unwrap();
      let prefix = &item.titles[0];
      let child = item.titles.get(1);

      let display_name = if let Some(child_title) = child {
        format!("{} / {}", clean_heading(prefix), title_case(&clean_heading(child_title)))
      } else {
        title_case(&clean_heading(prefix))
      };

      let fixture = FixtureForWrite {
        path: format!("{}/{}/{}", FIXTURE_PREFIX, section, file_name),
        name: display_name,
        checked: false,
        source: llhttp::Source {
          path: item.source_path,
          line: item.source_line,
        },
        meta: if has_meta(&item.meta) { item.meta } else { None },
        input: input.split('\n').map(String::from).collect(),
        llhttp: log.split('\n').map(String::from).collect(),
        output: None,
      };

      cases.push(CaseItem {
        path: file_name,
        fixture,
      });
    }
  }

  let target_dir = fixture_root.join(format!("{}s", section));
  fs::create_dir_all(&target_dir).unwrap();
  let temp_fixture_path = target_dir.join(format!(".import-llhttp-{}-temp.yml", section));

  let total = cases.len();
  for (i, item) in cases.iter_mut().enumerate() {
    println!("Processing {} case {}/{}: {}", section, i + 1, total, item.path);

    let file_path = target_dir.join(&item.path);
    seen_files.insert(format!("{}s/{}", section, item.path).replace('\\', "/"));

    // Write temporary fixture with empty output
    let temp_fixture = FixtureForWrite {
      path: item.fixture.path.clone(),
      name: item.fixture.name.clone(),
      checked: false,
      source: llhttp::Source {
        path: item.fixture.source.path.clone(),
        line: item.fixture.source.line,
      },
      meta: item.fixture.meta.clone(),
      input: item.fixture.input.clone(),
      llhttp: item.fixture.llhttp.clone(),
      output: Some(Vec::new()),
    };

    let initial_content = stringify_fixture(&temp_fixture);
    let content = if initial_content.ends_with('\n') {
      initial_content
    } else {
      format!("{}\n", initial_content)
    };
    fs::write(&temp_fixture_path, &content).unwrap();

    // Run generator
    let section_str = format!("{}s", section);
    let result = llhttp::run_test(&section_str, temp_fixture_path.to_str().unwrap());

    let output_events: Vec<llhttp::Event> = serde_yaml::from_str(&result.actual).unwrap();

    let final_fixture = FixtureForWrite {
      path: item.fixture.path.clone(),
      name: item.fixture.name.clone(),
      checked: false,
      source: llhttp::Source {
        path: item.fixture.source.path.clone(),
        line: item.fixture.source.line,
      },
      meta: item.fixture.meta.clone(),
      input: item.fixture.input.clone(),
      llhttp: item.fixture.llhttp.clone(),
      output: Some(output_events),
    };

    // Skip writes when only comments/checked differ semantically
    let skip_overwrite = file_path.exists()
      && fs::read_to_string(&file_path)
        .ok()
        .and_then(|existing_raw| {
          let existing = serde_yaml::from_str::<serde_yaml::Value>(&existing_raw).ok()?;
          let new_yaml_str = serde_yaml::to_string(&final_fixture).unwrap();
          let new_val = serde_yaml::from_str::<serde_yaml::Value>(&new_yaml_str).ok()?;
          Some(
            serde_json::to_string(&normalize_for_comparison(&existing)).unwrap()
              == serde_json::to_string(&normalize_for_comparison(&new_val)).unwrap(),
          )
        })
        .unwrap_or(false);

    if skip_overwrite {
      continue;
    }

    let final_content = stringify_fixture(&final_fixture);
    let content = if final_content.ends_with('\n') {
      final_content
    } else {
      format!("{}\n", final_content)
    };
    fs::write(&file_path, &content).unwrap();
  }

  let _ = fs::remove_file(&temp_fixture_path);
}

fn collect_md_files(dir: &Path, files: &mut Vec<PathBuf>) {
  if !dir.is_dir() {
    return;
  }

  let mut entries: Vec<_> = fs::read_dir(dir).unwrap().map(|e| e.unwrap()).collect();
  entries.sort_by_key(|e| e.path());

  for entry in entries {
    let path = entry.path();
    if path.is_dir() {
      collect_md_files(&path, files);
    } else if path.extension().and_then(|e| e.to_str()) == Some("md") {
      files.push(path);
    }
  }
}

pub fn import_tests(llhttp_root: &str) {
  let llhttp_root = Path::new(llhttp_root);
  let output_root = Path::new(".");
  let fixture_root = output_root.join(FIXTURE_PREFIX);

  if !llhttp_root.is_dir() {
    eprintln!("Error: llhttp root '{}' is not a directory", llhttp_root.display());
    std::process::exit(1);
  }

  fs::create_dir_all(&fixture_root).unwrap();

  // Snapshot existing fixtures
  let mut existing_files: HashSet<String> = HashSet::new();
  for section in &["requests", "responses"] {
    let section_root = fixture_root.join(section);
    if section_root.is_dir() {
      collect_yml_files(&section_root, section, &mut existing_files);
    }
  }

  let mut seen_files: HashSet<String> = HashSet::new();

  process_section(llhttp_root, output_root, &fixture_root, "request", &mut seen_files);
  process_section(llhttp_root, output_root, &fixture_root, "response", &mut seen_files);

  // Remove stale fixtures
  for file in &existing_files {
    if !seen_files.contains(file) {
      let path = fixture_root.join(file);
      let _ = fs::remove_file(&path);
      println!("Removed stale fixture: {}", file);
    }
  }
}

fn collect_yml_files(dir: &Path, prefix: &str, files: &mut HashSet<String>) {
  if !dir.is_dir() {
    return;
  }

  for entry in fs::read_dir(dir).unwrap() {
    let entry = entry.unwrap();
    let path = entry.path();
    if path.is_dir() {
      let new_prefix = format!("{}/{}", prefix, path.file_name().unwrap().to_str().unwrap());
      collect_yml_files(&path, &new_prefix, files);
    } else if path.extension().and_then(|e| e.to_str()) == Some("yml") {
      let name = path.file_name().unwrap().to_str().unwrap();
      files.insert(format!("{}/{}", prefix, name).replace('\\', "/"));
    }
  }
}

fn main() {
  let args: Vec<String> = std::env::args().collect();

  if args.len() < 2 {
    eprintln!("Usage: import_llhttp <llhttp-root>");
    std::process::exit(1);
  }

  import_tests(&args[1]);
}
