//! Parsing of the `x-kotlin-source` extension and helpers for deriving
//! Kotlin identifiers from OAS property names.

/// Parsed `x-kotlin-source` value.
///
/// Two shapes are accepted:
///
/// 1. Object (current): `{ "class": "ca.x.Outer.Inner", "module": "foo:bar" }`.
///    `class` is the full Kotlin reference (package + class chain). `module`
///    is a Gradle-style subproject path; colons map to `/` so
///    `foo:bar:baz` ⇒ `foo/bar/baz`. The on-disk file is computed:
///    `backend/<module-as-path>/src/main/kotlin/<lowercase-prefix-of-class-as-path>/<first-PascalCase-segment>.kt`.
/// 2. Legacy string: `"<path>#<fqcn>"`. Path is taken verbatim, FQCN's last
///    segment is the class name. Kept for backwards-compat with the original
///    fixture; new specs use the object shape.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct KotlinTarget {
    pub file: String,
    pub package: String,
    pub class_name: String,
}

impl KotlinTarget {
    pub fn parse(value: &serde_json::Value) -> Result<Self, String> {
        if let Some(s) = value.as_str() {
            return Self::parse_string(s);
        }
        if let Some(obj) = value.as_object() {
            return Self::parse_object(obj);
        }
        Err(format!(
            "x-kotlin-source must be a string or object, got {value}"
        ))
    }

    fn parse_object(obj: &serde_json::Map<String, serde_json::Value>) -> Result<Self, String> {
        let class = obj
            .get("class")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "x-kotlin-source object missing required 'class' string field".to_string())?
            .trim();
        let module = obj
            .get("module")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "x-kotlin-source object missing required 'module' string field".to_string())?
            .trim();
        if class.is_empty() {
            return Err("x-kotlin-source 'class' is empty".into());
        }
        if module.is_empty() {
            return Err("x-kotlin-source 'module' is empty".into());
        }

        // Split the class FQCN into (package-segments, class-chain).
        // The package portion is the lowercase-starting prefix; the first
        // segment starting with an uppercase letter opens the class chain.
        let segments: Vec<&str> = class.split('.').collect();
        let class_start = segments
            .iter()
            .position(|s| s.chars().next().map_or(false, |c| c.is_ascii_uppercase()))
            .ok_or_else(|| format!("x-kotlin-source class '{class}' has no class name (all-lowercase FQCN?)"))?;
        let outer_class = segments[class_start];
        let package_segments = &segments[..class_start];
        let class_chain = &segments[class_start..];

        // Validate every class-chain segment is a real Kotlin identifier.
        for seg in class_chain {
            if !is_valid_kotlin_identifier(seg) {
                return Err(format!(
                    "x-kotlin-source class '{class}' has invalid identifier segment '{seg}' \
                     (Kotlin type expressions like `List<T>` aren't supported — annotate the \
                     inner type and let arrays/maps inline automatically)"
                ));
            }
        }

        let leaf_class = class_chain
            .last()
            .expect("class_start guarantees ≥1 class segment");
        let parent_chain = &class_chain[..class_chain.len() - 1];
        let package = if parent_chain.is_empty() {
            package_segments.join(".")
        } else {
            // Re-attach the enclosing classes so `Outer.Inner` is stored as
            // `package="<pkg>.Outer", class_name="Inner"` — exactly the
            // representation the renderer's nesting tree already consumes.
            let mut p = package_segments.join(".");
            if !p.is_empty() && !parent_chain.is_empty() {
                p.push('.');
            }
            p.push_str(&parent_chain.join("."));
            p
        };

        let module_path = module.replace(':', "/");
        let package_dir = package_segments.join("/");
        let file = if package_dir.is_empty() {
            format!("backend/{module_path}/src/main/kotlin/{outer_class}.kt")
        } else {
            format!("backend/{module_path}/src/main/kotlin/{package_dir}/{outer_class}.kt")
        };

        Ok(KotlinTarget {
            file,
            package,
            class_name: leaf_class.to_string(),
        })
    }

    fn parse_string(raw: &str) -> Result<Self, String> {
        let raw = raw.trim();
        let Some((file, fqcn)) = raw.split_once('#') else {
            return Err(format!(
                "x-kotlin-source value '{raw}' is missing the '#<fqcn>' suffix"
            ));
        };
        let file = file.trim();
        let fqcn = fqcn.trim();
        if file.is_empty() {
            return Err(format!("x-kotlin-source '{raw}' has empty file path"));
        }
        if fqcn.is_empty() {
            return Err(format!("x-kotlin-source '{raw}' has empty FQCN"));
        }
        let (package, class_name) = match fqcn.rsplit_once('.') {
            Some((pkg, cls)) => (pkg.to_string(), cls.to_string()),
            None => {
                return Err(format!(
                    "x-kotlin-source FQCN '{fqcn}' must include a package (got bare class name)"
                ))
            }
        };
        if !is_valid_kotlin_identifier(&class_name) {
            return Err(format!(
                "x-kotlin-source FQCN '{fqcn}' has non-identifier class name '{class_name}' \
                 (Kotlin type expressions like `List<T>` aren't supported here — annotate \
                 the inner type and let the generator inline arrays/maps automatically)"
            ));
        }
        Ok(KotlinTarget {
            file: file.to_string(),
            package,
            class_name,
        })
    }

    pub fn fqcn(&self) -> String {
        format!("{}.{}", self.package, self.class_name)
    }
}

fn is_valid_kotlin_identifier(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    let mut chars = s.chars();
    let first = chars.next().unwrap();
    if !first.is_ascii_alphabetic() && first != '_' {
        return false;
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Turn an OAS property name into an idiomatic Kotlin identifier.
/// Returns `(kotlin_name, original_name)`. Caller emits `@SerialName(original)`
/// when the two differ.
pub fn property_ident(oas_name: &str) -> (String, String) {
    let kotlin = to_camel_case(oas_name);
    let kotlin = if is_reserved(&kotlin) {
        format!("`{kotlin}`")
    } else {
        kotlin
    };
    (kotlin, oas_name.to_string())
}

/// Convert `snake_case` / `kebab-case` / `PascalCase` / mixed to lowerCamelCase.
fn to_camel_case(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut upper_next = false;
    let mut first = true;
    for ch in input.chars() {
        if ch == '_' || ch == '-' || ch == ' ' {
            upper_next = !first;
            continue;
        }
        if first {
            out.push(ch.to_ascii_lowercase());
            first = false;
            upper_next = false;
            continue;
        }
        if upper_next {
            out.push(ch.to_ascii_uppercase());
            upper_next = false;
        } else {
            out.push(ch);
        }
    }
    if out.is_empty() {
        return "value".into();
    }
    if !out
        .chars()
        .next()
        .map(|c| c.is_ascii_alphabetic() || c == '_')
        .unwrap_or(false)
    {
        out.insert(0, '_');
    }
    out
}

/// Convert any string into an enum-variant style identifier (UPPER_SNAKE).
pub fn enum_variant_ident(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut prev_lower = false;
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() {
            if ch.is_ascii_uppercase() && prev_lower {
                out.push('_');
            }
            out.push(ch.to_ascii_uppercase());
            prev_lower = ch.is_ascii_lowercase();
        } else {
            if !out.ends_with('_') && !out.is_empty() {
                out.push('_');
            }
            prev_lower = false;
        }
    }
    let trimmed = out.trim_matches('_').to_string();
    if trimmed.is_empty() {
        return "VALUE".into();
    }
    if !trimmed
        .chars()
        .next()
        .map(|c| c.is_ascii_alphabetic() || c == '_')
        .unwrap_or(false)
    {
        format!("_{trimmed}")
    } else {
        trimmed
    }
}

fn is_reserved(name: &str) -> bool {
    matches!(
        name,
        "as" | "break"
            | "class"
            | "continue"
            | "do"
            | "else"
            | "false"
            | "for"
            | "fun"
            | "if"
            | "in"
            | "interface"
            | "is"
            | "null"
            | "object"
            | "package"
            | "return"
            | "super"
            | "this"
            | "throw"
            | "true"
            | "try"
            | "typealias"
            | "typeof"
            | "val"
            | "var"
            | "when"
            | "while"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn json_str(s: &str) -> serde_json::Value {
        serde_json::Value::String(s.into())
    }
    fn json_obj(class: &str, module: &str) -> serde_json::Value {
        serde_json::json!({ "class": class, "module": module })
    }

    #[test]
    fn parses_legacy_string_shape() {
        let t = KotlinTarget::parse(&json_str(
            "backend/x/src/main/kotlin/ca/dialai/flows/PublishedFlowResponseBody.kt#ca.dialai.flows.PublishedFlowResponseBody",
        ))
        .unwrap();
        assert_eq!(t.package, "ca.dialai.flows");
        assert_eq!(t.class_name, "PublishedFlowResponseBody");
        assert_eq!(
            t.file,
            "backend/x/src/main/kotlin/ca/dialai/flows/PublishedFlowResponseBody.kt"
        );
    }

    #[test]
    fn parses_object_shape_flat_class() {
        let t = KotlinTarget::parse(&json_obj(
            "ca.dialai.permissions.DialAiScope",
            "role-scopes-api",
        ))
        .unwrap();
        assert_eq!(t.package, "ca.dialai.permissions");
        assert_eq!(t.class_name, "DialAiScope");
        assert_eq!(
            t.file,
            "backend/role-scopes-api/src/main/kotlin/ca/dialai/permissions/DialAiScope.kt"
        );
    }

    #[test]
    fn parses_object_shape_nested_class() {
        let t = KotlinTarget::parse(&json_obj(
            "ca.dialai.session.auth.SessionAuthModule.CallbackRequest",
            "session-auth-http",
        ))
        .unwrap();
        // The enclosing class chain stays on the package side so the
        // renderer's nesting tree can recognise Outer.Inner via FQCN-prefix.
        assert_eq!(t.package, "ca.dialai.session.auth.SessionAuthModule");
        assert_eq!(t.class_name, "CallbackRequest");
        // File path always names the outermost class.
        assert_eq!(
            t.file,
            "backend/session-auth-http/src/main/kotlin/ca/dialai/session/auth/SessionAuthModule.kt"
        );
    }

    #[test]
    fn parses_object_shape_gradle_path_module() {
        let t = KotlinTarget::parse(&json_obj(
            "ca.dialai.customerjourney.http.IdentifierConfigResponse",
            "customer-journey:customer-journey-http",
        ))
        .unwrap();
        // Module colons map to slashes (Gradle sub-project conventions).
        assert_eq!(
            t.file,
            "backend/customer-journey/customer-journey-http/src/main/kotlin/ca/dialai/customerjourney/http/IdentifierConfigResponse.kt"
        );
    }

    #[test]
    fn rejects_object_missing_class() {
        assert!(KotlinTarget::parse(&serde_json::json!({"module": "app"})).is_err());
    }

    #[test]
    fn rejects_object_missing_module() {
        assert!(KotlinTarget::parse(&serde_json::json!({"class": "ca.x.Y"})).is_err());
    }

    #[test]
    fn rejects_bare_class_name() {
        assert!(KotlinTarget::parse(&json_str("Foo.kt#Foo")).is_err());
    }

    #[test]
    fn rejects_missing_separator() {
        assert!(KotlinTarget::parse(&json_str("Foo.kt")).is_err());
    }

    #[test]
    fn property_ident_camel_cases_snake() {
        assert_eq!(
            property_ident("created_at"),
            ("createdAt".into(), "created_at".into())
        );
        assert_eq!(property_ident("id"), ("id".into(), "id".into()));
    }

    #[test]
    fn property_ident_quotes_reserved() {
        assert_eq!(
            property_ident("class"),
            ("`class`".into(), "class".into())
        );
    }

    #[test]
    fn enum_variant_uppercases() {
        assert_eq!(enum_variant_ident("active"), "ACTIVE");
        assert_eq!(enum_variant_ident("in-stock"), "IN_STOCK");
        assert_eq!(enum_variant_ident("not_available"), "NOT_AVAILABLE");
    }
}
