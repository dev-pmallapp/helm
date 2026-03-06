use crate::property::*;

#[test]
fn property_value_as_u64() {
    assert_eq!(PropertyValue::UInt(42).as_u64(), Some(42));
    assert_eq!(PropertyValue::Int(10).as_u64(), Some(10));
    assert_eq!(PropertyValue::Int(-1).as_u64(), None);
    assert_eq!(PropertyValue::Str("x".into()).as_u64(), None);
}

#[test]
fn property_value_as_f64() {
    assert!((PropertyValue::Float(3.14).as_f64().unwrap() - 3.14).abs() < f64::EPSILON);
    assert_eq!(PropertyValue::UInt(100).as_f64(), Some(100.0));
}

#[test]
fn property_value_equality() {
    assert_eq!(PropertyValue::Bool(true), PropertyValue::Bool(true));
    assert_ne!(PropertyValue::UInt(1), PropertyValue::UInt(2));
}

#[test]
fn property_value_as_str_returns_string() {
    assert_eq!(PropertyValue::Str("hello".into()).as_str(), Some("hello"));
    assert_eq!(PropertyValue::UInt(1).as_str(), None);
    assert_eq!(PropertyValue::Bool(false).as_str(), None);
}

#[test]
fn property_value_bool_as_u64_is_none() {
    assert_eq!(PropertyValue::Bool(true).as_u64(), None);
    assert_eq!(PropertyValue::Bool(false).as_u64(), None);
}

#[test]
fn property_value_float_as_u64_is_none() {
    assert_eq!(PropertyValue::Float(1.5).as_u64(), None);
}

#[test]
fn property_value_int_as_f64() {
    assert_eq!(PropertyValue::Int(-10).as_f64(), Some(-10.0));
}

#[test]
fn property_value_str_as_f64_is_none() {
    assert_eq!(PropertyValue::Str("x".into()).as_f64(), None);
}

#[test]
fn property_construction_fields() {
    let p = Property {
        name: "width".into(),
        type_info: PropertyType::UInt { min: Some(1), max: Some(16) },
        description: "pipeline width".into(),
        read_only: false,
    };
    assert_eq!(p.name, "width");
    assert!(!p.read_only);
}

#[test]
fn property_read_only_flag() {
    let p = Property {
        name: "version".into(),
        type_info: PropertyType::Str,
        description: "hw version".into(),
        read_only: true,
    };
    assert!(p.read_only);
}

#[test]
fn property_value_roundtrips_through_json() {
    let vals = vec![
        PropertyValue::Bool(true),
        PropertyValue::Int(-42),
        PropertyValue::UInt(128),
        PropertyValue::Float(2.5),
        PropertyValue::Str("hello".into()),
    ];
    for v in vals {
        let json = serde_json::to_string(&v).unwrap();
        let back: PropertyValue = serde_json::from_str(&json).unwrap();
        assert_eq!(v, back);
    }
}
