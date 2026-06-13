use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

fn main() {
  let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
  let root = manifest_dir.parent().unwrap();

  // Read version from parser/Cargo.toml
  let cargo_toml = fs::read_to_string(manifest_dir.join("Cargo.toml")).unwrap();
  let cargo: toml::Value = toml::from_str(&cargo_toml).unwrap();
  let version_str = cargo["package"]["version"].as_str().unwrap();
  let version = semver::Version::parse(version_str).unwrap();

  // Read YAML constants
  let methods: Vec<String> =
    serde_yaml::from_str(&fs::read_to_string(root.join("macros/constants/methods.yml")).unwrap()).unwrap();
  let errors: Vec<String> =
    serde_yaml::from_str(&fs::read_to_string(root.join("macros/constants/errors.yml")).unwrap()).unwrap();
  let callbacks: Vec<String> =
    serde_yaml::from_str(&fs::read_to_string(root.join("macros/constants/callbacks.yml")).unwrap()).unwrap();
  let states: Vec<String> =
    serde_yaml::from_str(&fs::read_to_string(root.join("macros/constants/states.yml")).unwrap()).unwrap();

  // Build constants map
  let mut constants = BTreeMap::new();

  for (i, method) in methods.iter().enumerate() {
    constants.insert(format!("METHOD_{}", method.replace('-', "_")), serde_json::Value::from(i));
  }

  for (i, callback) in callbacks.iter().enumerate() {
    constants.insert(format!("CALLBACK_{}", callback.to_uppercase()), serde_json::Value::from(i));
  }

  let mut all: u64 = 0;
  constants.insert("CALLBACK_ACTIVE_NONE".into(), serde_json::Value::from(0));
  for (i, callback) in callbacks.iter().enumerate() {
    let bit: u64 = 1 << i;
    constants.insert(format!("CALLBACK_ACTIVE_{}", callback.to_uppercase()), serde_json::Value::from(bit));
    all |= bit;
  }
  constants.insert("CALLBACK_ACTIVE_ALL".into(), serde_json::Value::from(all));

  for (i, error) in errors.iter().enumerate() {
    constants.insert(format!("ERROR_{}", error), serde_json::Value::from(i));
  }

  for (i, state) in states.iter().enumerate() {
    constants.insert(format!("STATE_{}", state.to_uppercase()), serde_json::Value::from(i));
  }

  let output = serde_json::json!({
    "version": {
      "raw": version.to_string(),
      "major": version.major,
      "minor": version.minor,
      "patch": version.patch,
      "prerelease": version.pre.to_string(),
    },
    "constants": constants,
  });

  println!("{}", serde_json::to_string_pretty(&output).unwrap());
}
