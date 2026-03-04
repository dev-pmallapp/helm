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
