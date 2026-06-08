use serde::Deserialize;

/// Routing policy for multi-provider embedding requests.
///
/// - `All`: every provider in the list must succeed; partial success is a
///   failure (502 returned alongside diagnostics).
/// - `Any`: at least one provider must succeed; partial failures are reported
///   alongside the successful results (200 returned).
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum RoutingPolicy {
    All,
    #[default]
    Any,
}

// ── Unit tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_policy_deserialization() {
        let all: RoutingPolicy =
            serde_json::from_str("\"all\"").expect("should deserialize 'all'");
        assert_eq!(all, RoutingPolicy::All);

        let any: RoutingPolicy =
            serde_json::from_str("\"any\"").expect("should deserialize 'any'");
        assert_eq!(any, RoutingPolicy::Any);
    }

    #[test]
    fn test_policy_default() {
        let policy = RoutingPolicy::default();
        assert_eq!(policy, RoutingPolicy::Any, "default policy must be Any");
    }

    #[test]
    fn test_policy_invalid_deserializes_to_error() {
        let result: Result<RoutingPolicy, _> = serde_json::from_str("\"round-robin\"");
        assert!(result.is_err(), "unknown policy variant should fail to deserialize");
    }

    #[test]
    fn test_policy_clone() {
        let original = RoutingPolicy::All;
        let cloned = original.clone();
        assert_eq!(original, cloned);
    }

    #[test]
    fn test_policy_debug() {
        let all = format!("{:?}", RoutingPolicy::All);
        let any = format!("{:?}", RoutingPolicy::Any);
        assert!(all.contains("All"));
        assert!(any.contains("Any"));
    }
}
