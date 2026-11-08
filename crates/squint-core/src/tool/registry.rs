//! The tool registry for one run: the enabled tools in prompt order, with lookup by name.

use std::sync::Arc;

use super::{Manifest, Tool};
use crate::{Error, Result};

pub struct ToolRegistry {
    tools: Vec<Arc<dyn Tool>>,
    /// The projection each tool runs under, by name; absent means the full envelope.
    projections: std::collections::BTreeMap<String, String>,
    /// The lock digest of the installed bundle each pack tool came from, by tool name: part of
    /// the cache key and the evaluation descriptor. A built-in, or a pack run by `command`, has
    /// none.
    lock_digests: std::collections::BTreeMap<String, String>,
}

impl ToolRegistry {
    /// Keeps the tools named in `enabled`, ordered by `order` where given, then by cost tier
    /// (cheap first) with `requires` before dependants.
    pub fn new(
        mut available: Vec<Box<dyn Tool>>,
        enabled: &[String],
        order: &[String],
    ) -> Result<ToolRegistry> {
        let mut tools: Vec<Arc<dyn Tool>> = Vec::new();
        for name in enabled {
            let pos = available
                .iter()
                .position(|t| &t.manifest().name == name)
                .ok_or_else(|| {
                    Error::Config(format!(
                        "tool {name:?} is enabled but not available in this build"
                    ))
                })?;
            tools.push(Arc::from(available.remove(pos)));
        }
        for t in &tools {
            for req in &t.manifest().requires {
                if !tools.iter().any(|o| &o.manifest().name == req) {
                    return Err(Error::Config(format!(
                        "tool {:?} requires {req:?}, which is not enabled",
                        t.manifest().name
                    )));
                }
            }
        }
        tools.sort_by_key(|t| {
            let m = t.manifest();
            let explicit = order.iter().position(|o| o == &m.name);
            let tier = m.cost.tier as u8;
            (
                explicit.is_none(),
                explicit.unwrap_or(0),
                tier,
                m.name.clone(),
            )
        });
        // Requires before dependants: stable pass moving prerequisites ahead.
        let mut i = 0;
        while i < tools.len() {
            let reqs = tools[i].manifest().requires.clone();
            if let Some(j) = reqs
                .iter()
                .filter_map(|r| tools.iter().position(|t| &t.manifest().name == r))
                .find(|&j| j > i)
            {
                let dep = tools.remove(j);
                tools.insert(i, dep);
                continue;
            }
            i += 1;
        }
        Ok(ToolRegistry {
            tools,
            projections: std::collections::BTreeMap::new(),
            lock_digests: std::collections::BTreeMap::new(),
        })
    }

    /// Records the lock digest of the installed bundle each named tool came from.
    pub fn with_lock_digests(
        mut self,
        digests: std::collections::BTreeMap<String, String>,
    ) -> ToolRegistry {
        self.lock_digests = digests;
        self
    }

    /// The lock digests recorded with `with_lock_digests`, by tool name.
    pub fn lock_digests(&self) -> &std::collections::BTreeMap<String, String> {
        &self.lock_digests
    }

    /// Runs the named tools under the named projections (SPEC "Evaluation projections"): every
    /// envelope they return is projected before the run sees it. A tool that is not enabled, or
    /// does not declare the projection, is a configuration error.
    pub fn with_projections(
        mut self,
        projections: &std::collections::BTreeMap<String, String>,
    ) -> Result<ToolRegistry> {
        for (name, projection) in projections {
            let pos = self
                .tools
                .iter()
                .position(|t| &t.manifest().name == name)
                .ok_or_else(|| {
                    Error::Config(format!(
                        "projection {projection:?} names tool {name:?}, which is not enabled"
                    ))
                })?;
            let inner = Arc::clone(&self.tools[pos]);
            self.tools[pos] = Arc::new(super::Projected::new(inner, projection)?);
            self.projections.insert(name.clone(), projection.clone());
        }
        Ok(self)
    }

    /// The projection a tool runs under, if any.
    pub fn projection_of(&self, name: &str) -> Option<&str> {
        self.projections.get(name).map(String::as_str)
    }

    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    pub fn len(&self) -> usize {
        self.tools.len()
    }

    pub fn manifests(&self) -> impl Iterator<Item = &Manifest> {
        self.tools.iter().map(|t| t.manifest())
    }

    pub fn get(&self, name: &str) -> Option<&dyn Tool> {
        self.tools
            .iter()
            .find(|t| t.manifest().name == name)
            .map(|t| t.as_ref())
    }

    /// The same tools narrowed to `enabled`, in this registry's order, sharing the running tools
    /// (a pack is not started again), with their projections and lock digests: one call's tool set
    /// inside a session's. A name this registry lacks, or a kept tool whose prerequisite was
    /// dropped, is a configuration error.
    pub fn narrowed(&self, enabled: &[String]) -> Result<ToolRegistry> {
        if let Some(missing) = enabled.iter().find(|n| self.get(n).is_none()) {
            return Err(Error::Config(format!(
                "tool {missing:?} is not enabled in this configuration"
            )));
        }
        let tools: Vec<Arc<dyn Tool>> = self
            .tools
            .iter()
            .filter(|t| enabled.contains(&t.manifest().name))
            .cloned()
            .collect();
        for t in &tools {
            for req in &t.manifest().requires {
                if !tools.iter().any(|o| &o.manifest().name == req) {
                    return Err(Error::Config(format!(
                        "tool {:?} requires {req:?}, which is not enabled",
                        t.manifest().name
                    )));
                }
            }
        }
        let keep = |name: &String| enabled.contains(name);
        Ok(ToolRegistry {
            tools,
            projections: self
                .projections
                .iter()
                .filter(|(n, _)| keep(n))
                .map(|(n, p)| (n.clone(), p.clone()))
                .collect(),
            lock_digests: self
                .lock_digests
                .iter()
                .filter(|(n, _)| keep(n))
                .map(|(n, d)| (n.clone(), d.clone()))
                .collect(),
        })
    }

    /// A shared handle, for running a call on another thread.
    pub fn get_shared(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools
            .iter()
            .find(|t| t.manifest().name == name)
            .cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::{Envelope, SquintContext};

    struct Named(Manifest);
    impl Tool for Named {
        fn manifest(&self) -> &Manifest {
            &self.0
        }
        fn call(&self, _: &serde_json::Value, _: &SquintContext) -> Result<Envelope> {
            Err(Error::Tool("unused".into()))
        }
    }

    fn tool(name: &str, tier: &str, requires: &[&str]) -> Box<dyn Tool> {
        let text = crate::fixture("tools/regions.toml")
            .replace("name = \"regions\"", &format!("name = \"{name}\""))
            .replace("tier = \"expensive\"", &format!("tier = \"{tier}\""))
            .replace(
                "requires = [\"align\"]",
                &format!(
                    "requires = [{}]",
                    requires
                        .iter()
                        .map(|r| format!("\"{r}\""))
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            );
        Box::new(Named(Manifest::from_toml(&text).expect("manifest")))
    }

    #[test]
    fn default_order_is_cheap_first_with_requirements_ahead() {
        let reg = ToolRegistry::new(
            vec![
                tool("regions", "expensive", &["align"]),
                tool("view", "cheap", &[]),
                tool("align", "moderate", &[]),
            ],
            &["regions".into(), "view".into(), "align".into()],
            &[],
        )
        .expect("registry");
        let names: Vec<&str> = reg.manifests().map(|m| m.name.as_str()).collect();
        assert_eq!(names, ["view", "align", "regions"]);
    }

    #[test]
    fn explicit_order_wins_but_requirements_still_come_first() {
        let reg = ToolRegistry::new(
            vec![
                tool("regions", "expensive", &["align"]),
                tool("view", "cheap", &[]),
                tool("align", "moderate", &[]),
            ],
            &["regions".into(), "view".into(), "align".into()],
            &["regions".into(), "view".into()],
        )
        .expect("registry");
        let names: Vec<&str> = reg.manifests().map(|m| m.name.as_str()).collect();
        assert_eq!(names, ["align", "regions", "view"]);
    }

    #[test]
    fn projections_wrap_declared_tools_and_refuse_the_rest() {
        let declared = |name: &str| -> Box<dyn Tool> {
            let text = crate::fixture("tools/regions.toml")
                .replace("name = \"regions\"", &format!("name = \"{name}\""))
                .replace("requires = [\"align\"]", "requires = []")
                .replace(
                    "concurrent = false",
                    "concurrent = false\nprojections = [\"visual_only\"]",
                );
            Box::new(Named(Manifest::from_toml(&text).expect("manifest")))
        };
        let projections = |pairs: &[(&str, &str)]| {
            pairs
                .iter()
                .map(|(t, p)| (t.to_string(), p.to_string()))
                .collect::<std::collections::BTreeMap<_, _>>()
        };
        let reg = ToolRegistry::new(
            vec![declared("align"), tool("view", "cheap", &[])],
            &["align".into(), "view".into()],
            &[],
        )
        .expect("registry")
        .with_projections(&projections(&[("align", "visual_only")]))
        .expect("projected");
        assert_eq!(reg.projection_of("align"), Some("visual_only"));
        assert_eq!(reg.projection_of("view"), None);
        assert_eq!(
            reg.get("align").map(|t| t.manifest().name.as_str()),
            Some("align")
        );
        // `view` does not declare the projection; a tool that is not enabled cannot be projected;
        // a projection the harness does not know is refused.
        for (tool_name, projection) in [
            ("view", "visual_only"),
            ("regions", "visual_only"),
            ("align", "blurry"),
        ] {
            let reg = ToolRegistry::new(
                vec![declared("align"), tool("view", "cheap", &[])],
                &["align".into(), "view".into()],
                &[],
            )
            .expect("registry");
            assert!(
                matches!(
                    reg.with_projections(&projections(&[(tool_name, projection)])),
                    Err(Error::Config(_))
                ),
                "{tool_name} under {projection}"
            );
        }
    }

    #[test]
    fn missing_requirement_and_unknown_tool_are_errors() {
        assert!(
            ToolRegistry::new(
                vec![tool("regions", "expensive", &["align"])],
                &["regions".into()],
                &[]
            )
            .is_err()
        );
        assert!(ToolRegistry::new(vec![], &["nope".into()], &[]).is_err());
    }
}
