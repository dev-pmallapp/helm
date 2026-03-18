//! Device parameter schema and value types.
//!
//! Declares the configuration surface for device types. Every device type
//! exposes a [`ParamSchema`] that lists its accepted parameters, types, defaults,
//! and documentation strings. At instantiation time, a [`DeviceParams`] map is
//! validated against the schema before being passed to the device factory.
//!
//! # Two-phase validation
//!
//! - **Assignment-time** (`ParamSchema::validate`): checks type correctness and
//!   required-field presence. Called when Python assigns parameters (before
//!   `elaborate()`).
//! - **Realize-time** (device factory): semantic validation that requires
//!   cross-field reasoning (e.g. "FIFO depth must be a power of two").

use std::collections::HashMap;

use super::registry::DldError;

// ---- ParamType ----------------------------------------------------------

/// The type of a device parameter field.
#[derive(Debug, Clone)]
pub enum ParamType {
    /// Signed 64-bit integer.
    Int,
    /// Boolean.
    Bool,
    /// Memory size, parsed from strings like "32KiB", "4MiB", or plain integer bytes.
    MemorySize,
    /// UTF-8 string.
    String,
    /// One of a fixed set of named values.
    Enum(&'static [&'static str]),
}

// ---- ParamValue ---------------------------------------------------------

/// A concrete parameter value.
#[derive(Debug, Clone)]
pub enum ParamValue {
    /// Signed 64-bit integer.
    Int(i64),
    /// Boolean.
    Bool(bool),
    /// Memory size in bytes.
    MemorySize(u64),
    /// UTF-8 string.
    String(std::string::String),
    /// Index into `ParamType::Enum` variants.
    Enum(u32),
}

impl From<i64> for ParamValue {
    fn from(v: i64) -> Self {
        Self::Int(v)
    }
}

impl From<bool> for ParamValue {
    fn from(v: bool) -> Self {
        Self::Bool(v)
    }
}

impl From<&str> for ParamValue {
    fn from(v: &str) -> Self {
        Self::String(v.to_owned())
    }
}

// ---- ParamField ---------------------------------------------------------

/// Description of one parameter field in a device's configuration.
#[derive(Debug, Clone)]
pub struct ParamField {
    /// Parameter name -- used as the key in `DeviceParams`.
    pub name: &'static str,
    /// Type and valid values.
    pub kind: ParamType,
    /// Default value. Applied if the parameter is absent from `DeviceParams`.
    pub default: ParamValue,
    /// Whether this parameter is required. If `true` and absent with no
    /// sensible default, `DeviceRegistry::create()` returns `MissingParam`.
    pub required: bool,
    /// Human-readable description for Python `help()` output.
    pub description: &'static str,
}

// ---- ParamSchema --------------------------------------------------------

/// The complete parameter schema for a device type.
///
/// Declares every parameter the device accepts. Used by:
/// - **Python**: to validate attribute assignments before `elaborate()`
/// - **`DeviceRegistry`**: to apply defaults and validate before calling the factory
/// - **Python `help()`**: to display parameter documentation
#[derive(Debug, Clone)]
pub struct ParamSchema {
    fields: Vec<ParamField>,
}

impl ParamSchema {
    /// Create an empty schema (no parameters).
    pub fn new() -> Self {
        Self { fields: Vec::new() }
    }

    /// Add a required integer parameter.
    pub fn int(mut self, name: &'static str, description: &'static str) -> Self {
        self.fields.push(ParamField {
            name,
            description,
            kind: ParamType::Int,
            default: ParamValue::Int(0),
            required: true,
        });
        self
    }

    /// Add an optional integer parameter with a default value.
    pub fn int_default(
        mut self,
        name: &'static str,
        default: i64,
        description: &'static str,
    ) -> Self {
        self.fields.push(ParamField {
            name,
            description,
            kind: ParamType::Int,
            default: ParamValue::Int(default),
            required: false,
        });
        self
    }

    /// Add an optional boolean parameter with a default value.
    pub fn bool_default(
        mut self,
        name: &'static str,
        default: bool,
        description: &'static str,
    ) -> Self {
        self.fields.push(ParamField {
            name,
            description,
            kind: ParamType::Bool,
            default: ParamValue::Bool(default),
            required: false,
        });
        self
    }

    /// Add an optional memory-size parameter (stored as bytes).
    pub fn memory_size(
        mut self,
        name: &'static str,
        default_bytes: u64,
        description: &'static str,
    ) -> Self {
        self.fields.push(ParamField {
            name,
            description,
            kind: ParamType::MemorySize,
            default: ParamValue::MemorySize(default_bytes),
            required: false,
        });
        self
    }

    /// Add an enum parameter.
    ///
    /// `variants` is the list of valid string names. `default_idx` is the
    /// zero-based index into `variants` used when the parameter is absent.
    pub fn enum_param(
        mut self,
        name: &'static str,
        variants: &'static [&'static str],
        default_idx: u32,
        description: &'static str,
    ) -> Self {
        self.fields.push(ParamField {
            name,
            description,
            kind: ParamType::Enum(variants),
            default: ParamValue::Enum(default_idx),
            required: false,
        });
        self
    }

    /// Validate a [`DeviceParams`] map against this schema.
    ///
    /// Applies defaults for missing optional fields. Returns the
    /// validated/defaulted params, or a [`DldError`] on failure.
    ///
    /// This is **assignment-time validation**: checks presence of required
    /// fields and fills in defaults. Type checking is not performed here
    /// because `ParamValue` already carries its type tag; the factory
    /// function's typed getters (`get_int`, `get_bool`, ...) catch mismatches.
    pub fn validate(&self, mut params: DeviceParams) -> Result<DeviceParams, DldError> {
        for field in &self.fields {
            if !params.contains(field.name) {
                if field.required {
                    return Err(DldError::MissingParam(field.name));
                }
                params.insert(field.name, field.default.clone());
            }
        }
        Ok(params)
    }

    /// Borrow the list of declared fields.
    pub fn fields(&self) -> &[ParamField] {
        &self.fields
    }
}

impl Default for ParamSchema {
    fn default() -> Self {
        Self::new()
    }
}

// ---- DeviceParams -------------------------------------------------------

/// A concrete set of parameter values for one device instantiation.
///
/// Created by the Python config layer from keyword arguments.
/// Validated against [`ParamSchema`] before being passed to the device factory.
#[derive(Debug, Default, Clone)]
pub struct DeviceParams {
    values: HashMap<std::string::String, ParamValue>,
}

impl DeviceParams {
    /// Create an empty parameter set.
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert a parameter value by name.
    pub fn insert(&mut self, name: &str, val: ParamValue) {
        self.values.insert(name.to_owned(), val);
    }

    /// Check whether a parameter with the given name exists.
    pub fn contains(&self, name: &str) -> bool {
        self.values.contains_key(name)
    }

    /// Get an integer parameter by name. Returns `Err` if absent or wrong type.
    pub fn get_int(&self, name: &str) -> Result<i64, DldError> {
        match self.values.get(name) {
            Some(ParamValue::Int(v)) => Ok(*v),
            Some(_) => Err(DldError::WrongParamType(name.to_owned())),
            None => Err(DldError::MissingParam(
                // SAFETY: this leak is bounded -- only called for programmer-visible
                // parameter names during device construction (not in hot loop).
                Box::leak(name.to_owned().into_boxed_str()),
            )),
        }
    }

    /// Get a boolean parameter by name. Returns `Err` if absent or wrong type.
    pub fn get_bool(&self, name: &str) -> Result<bool, DldError> {
        match self.values.get(name) {
            Some(ParamValue::Bool(v)) => Ok(*v),
            Some(_) => Err(DldError::WrongParamType(name.to_owned())),
            None => Err(DldError::MissingParam(
                Box::leak(name.to_owned().into_boxed_str()),
            )),
        }
    }

    /// Get a memory-size parameter in bytes. Returns `Err` if absent or wrong type.
    pub fn get_memory_size(&self, name: &str) -> Result<u64, DldError> {
        match self.values.get(name) {
            Some(ParamValue::MemorySize(v)) => Ok(*v),
            Some(_) => Err(DldError::WrongParamType(name.to_owned())),
            None => Err(DldError::MissingParam(
                Box::leak(name.to_owned().into_boxed_str()),
            )),
        }
    }

    /// Get a string parameter by name. Returns `Err` if absent or wrong type.
    pub fn get_str(&self, name: &str) -> Result<&str, DldError> {
        match self.values.get(name) {
            Some(ParamValue::String(s)) => Ok(s.as_str()),
            Some(_) => Err(DldError::WrongParamType(name.to_owned())),
            None => Err(DldError::MissingParam(
                Box::leak(name.to_owned().into_boxed_str()),
            )),
        }
    }

    /// Parse a memory-size string into bytes.
    ///
    /// Supports binary SI suffixes (`KiB`, `MiB`, `GiB`) and decimal SI
    /// suffixes (`KB`, `MB`, `GB`). A plain integer is interpreted as bytes.
    ///
    /// # Examples
    ///
    /// ```
    /// # use helm_devices::framework::params::DeviceParams;
    /// assert_eq!(DeviceParams::parse_memory_size("4096").unwrap(), 4096);
    /// assert_eq!(DeviceParams::parse_memory_size("32KiB").unwrap(), 32 * 1024);
    /// assert_eq!(DeviceParams::parse_memory_size("4MiB").unwrap(), 4 * 1024 * 1024);
    /// assert_eq!(DeviceParams::parse_memory_size("1GiB").unwrap(), 1024 * 1024 * 1024);
    /// assert_eq!(DeviceParams::parse_memory_size("1GB").unwrap(), 1_000_000_000);
    /// ```
    pub fn parse_memory_size(s: &str) -> Result<u64, DldError> {
        let s = s.trim();
        let (num_part, mult) = if let Some(n) = s.strip_suffix("GiB") {
            (n, 1u64 << 30)
        } else if let Some(n) = s.strip_suffix("MiB") {
            (n, 1u64 << 20)
        } else if let Some(n) = s.strip_suffix("KiB") {
            (n, 1u64 << 10)
        } else if let Some(n) = s.strip_suffix("GB") {
            (n, 1_000_000_000u64)
        } else if let Some(n) = s.strip_suffix("MB") {
            (n, 1_000_000u64)
        } else if let Some(n) = s.strip_suffix("KB") {
            (n, 1_000u64)
        } else {
            (s, 1u64)
        };
        let n: u64 = num_part
            .trim()
            .parse()
            .map_err(|_| DldError::InvalidParamValue(format!("not a valid memory size: {s}")))?;
        Ok(n * mult)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_memory_size_plain_bytes() {
        assert_eq!(DeviceParams::parse_memory_size("4096").unwrap(), 4096);
    }

    #[test]
    fn parse_memory_size_kib() {
        assert_eq!(DeviceParams::parse_memory_size("32KiB").unwrap(), 32 * 1024);
    }

    #[test]
    fn parse_memory_size_mib() {
        assert_eq!(
            DeviceParams::parse_memory_size("4MiB").unwrap(),
            4 * 1024 * 1024
        );
    }

    #[test]
    fn parse_memory_size_gib() {
        assert_eq!(
            DeviceParams::parse_memory_size("1GiB").unwrap(),
            1024 * 1024 * 1024
        );
    }

    #[test]
    fn parse_memory_size_decimal_gb() {
        assert_eq!(
            DeviceParams::parse_memory_size("1GB").unwrap(),
            1_000_000_000
        );
    }

    #[test]
    fn parse_memory_size_invalid() {
        assert!(DeviceParams::parse_memory_size("not_a_number").is_err());
    }

    #[test]
    fn schema_validate_applies_defaults() {
        let schema = ParamSchema::new()
            .int("required_field", "a required int")
            .int_default("optional_field", 42, "an optional int");

        let mut params = DeviceParams::new();
        params.insert("required_field", ParamValue::Int(10));

        let validated = schema.validate(params).unwrap();
        assert_eq!(validated.get_int("required_field").unwrap(), 10);
        assert_eq!(validated.get_int("optional_field").unwrap(), 42);
    }

    #[test]
    fn schema_validate_missing_required() {
        let schema = ParamSchema::new().int("required_field", "a required int");
        let params = DeviceParams::new();
        assert!(schema.validate(params).is_err());
    }

    #[test]
    fn param_value_from_i64() {
        let v: ParamValue = 42i64.into();
        match v {
            ParamValue::Int(n) => assert_eq!(n, 42),
            _ => panic!("expected Int"),
        }
    }

    #[test]
    fn param_value_from_bool() {
        let v: ParamValue = true.into();
        match v {
            ParamValue::Bool(b) => assert!(b),
            _ => panic!("expected Bool"),
        }
    }

    #[test]
    fn param_value_from_str() {
        let v: ParamValue = "hello".into();
        match v {
            ParamValue::String(s) => assert_eq!(s, "hello"),
            _ => panic!("expected String"),
        }
    }

    #[test]
    fn device_params_get_wrong_type() {
        let mut params = DeviceParams::new();
        params.insert("x", ParamValue::Bool(true));
        assert!(params.get_int("x").is_err());
    }

    #[test]
    fn bool_default_schema() {
        let schema = ParamSchema::new().bool_default("flag", false, "a flag");
        let params = DeviceParams::new();
        let validated = schema.validate(params).unwrap();
        assert!(!validated.get_bool("flag").unwrap());
    }

    #[test]
    fn memory_size_schema() {
        let schema = ParamSchema::new().memory_size("size", 4096, "buffer size");
        let params = DeviceParams::new();
        let validated = schema.validate(params).unwrap();
        assert_eq!(validated.get_memory_size("size").unwrap(), 4096);
    }

    #[test]
    fn enum_schema() {
        let schema = ParamSchema::new().enum_param(
            "mode",
            &["fast", "slow", "auto"],
            2,
            "operation mode",
        );
        let fields = schema.fields();
        assert_eq!(fields.len(), 1);
        assert_eq!(fields[0].name, "mode");
    }

    #[test]
    fn get_str_param() {
        let mut params = DeviceParams::new();
        params.insert("name", ParamValue::String("uart".to_owned()));
        assert_eq!(params.get_str("name").unwrap(), "uart");
    }
}
