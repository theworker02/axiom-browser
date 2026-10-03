//! Element attributes (DOM §4.9.2): an ordered list of namespaced name/value pairs.

/// One attribute. `prefix` is only ever set together with `namespace`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attr {
    pub namespace: Option<String>,
    pub prefix: Option<String>,
    pub local_name: String,
    pub value: String,
}

impl Attr {
    /// `prefix:localName`, or the local name when there is no prefix.
    pub fn qualified_name(&self) -> String {
        match &self.prefix {
            Some(p) => format!("{p}:{}", self.local_name),
            None => self.local_name.clone(),
        }
    }

    pub fn has_qualified_name(&self, name: &str) -> bool {
        match &self.prefix {
            None => self.local_name == name,
            Some(p) => name
                .strip_prefix(p.as_str())
                .and_then(|rest| rest.strip_prefix(':'))
                .is_some_and(|local| local == self.local_name),
        }
    }

    fn is(&self, namespace: Option<&str>, local_name: &str) -> bool {
        self.namespace.as_deref() == namespace && self.local_name == local_name
    }
}

/// An element's attribute list, in the order attributes were added.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Attributes {
    list: Vec<Attr>,
}

impl Attributes {
    pub fn iter(&self) -> std::slice::Iter<'_, Attr> {
        self.list.iter()
    }

    pub fn len(&self) -> usize {
        self.list.len()
    }

    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }

    /// The value of the first attribute whose qualified name is `name`.
    pub fn get(&self, name: &str) -> Option<&str> {
        self.list
            .iter()
            .find(|a| a.has_qualified_name(name))
            .map(|a| a.value.as_str())
    }

    pub fn get_ns(&self, namespace: Option<&str>, local_name: &str) -> Option<&str> {
        self.list
            .iter()
            .find(|a| a.is(namespace, local_name))
            .map(|a| a.value.as_str())
    }

    pub fn contains_key(&self, name: &str) -> bool {
        self.get(name).is_some()
    }

    /// Changes the first attribute whose qualified name is `name`, or appends a
    /// no-namespace attribute with that local name.
    pub fn set(&mut self, name: &str, value: &str) {
        match self.list.iter_mut().find(|a| a.has_qualified_name(name)) {
            Some(a) => a.value = value.to_string(),
            None => self.list.push(Attr {
                namespace: None,
                prefix: None,
                local_name: name.to_string(),
                value: value.to_string(),
            }),
        }
    }

    /// Changes the attribute `(namespace, local_name)`, keeping its prefix, or appends
    /// a new one ("set an attribute value", DOM §4.9).
    pub fn set_ns(
        &mut self,
        namespace: Option<&str>,
        prefix: Option<&str>,
        local_name: &str,
        value: &str,
    ) {
        match self.list.iter_mut().find(|a| a.is(namespace, local_name)) {
            Some(a) => a.value = value.to_string(),
            None => self.list.push(Attr {
                namespace: namespace.map(str::to_string),
                prefix: prefix.map(str::to_string),
                local_name: local_name.to_string(),
                value: value.to_string(),
            }),
        }
    }

    /// Replaces the attribute `(namespace, local_name)` in place, prefix included, or
    /// appends it ("replace an attribute" / "append an attribute", DOM §4.9). Returns
    /// the replaced attribute.
    pub fn replace_ns(
        &mut self,
        namespace: Option<&str>,
        prefix: Option<&str>,
        local_name: &str,
        value: &str,
    ) -> Option<Attr> {
        let new = Attr {
            namespace: namespace.map(str::to_string),
            prefix: prefix.map(str::to_string),
            local_name: local_name.to_string(),
            value: value.to_string(),
        };
        match self.list.iter_mut().find(|a| a.is(namespace, local_name)) {
            Some(a) => Some(std::mem::replace(a, new)),
            None => {
                self.list.push(new);
                None
            }
        }
    }

    /// Removes the first attribute whose qualified name is `name`.
    pub fn remove(&mut self, name: &str) -> Option<Attr> {
        let i = self.list.iter().position(|a| a.has_qualified_name(name))?;
        Some(self.list.remove(i))
    }

    pub fn remove_ns(&mut self, namespace: Option<&str>, local_name: &str) -> Option<Attr> {
        let i = self.list.iter().position(|a| a.is(namespace, local_name))?;
        Some(self.list.remove(i))
    }
}

impl<'a> IntoIterator for &'a Attributes {
    type Item = &'a Attr;
    type IntoIter = std::slice::Iter<'a, Attr>;

    fn into_iter(self) -> Self::IntoIter {
        self.list.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_insertion_order_and_matches_qualified_names() {
        let mut a = Attributes::default();
        a.set("b", "1");
        a.set_ns(Some("urn:x"), Some("p"), "a", "2");
        a.set("a", "3");
        let names: Vec<String> = a.iter().map(Attr::qualified_name).collect();
        assert_eq!(names, ["b", "p:a", "a"]);
        assert_eq!(a.get("p:a"), Some("2"));
        assert_eq!(a.get("a"), Some("3"));
        assert_eq!(a.get_ns(Some("urn:x"), "a"), Some("2"));
        a.set_ns(Some("urn:x"), Some("q"), "a", "4");
        assert_eq!(a.get("p:a"), Some("4"), "set_ns keeps the existing prefix");
        assert_eq!(a.remove("p:a").map(|x| x.value), Some("4".into()));
        assert_eq!(a.len(), 2);
    }

    #[test]
    fn replace_ns_keeps_the_position_and_takes_the_new_prefix() {
        let mut a = Attributes::default();
        a.set_ns(Some("urn:x"), Some("p"), "a", "1");
        a.set("b", "2");
        let old = a.replace_ns(Some("urn:x"), Some("q"), "a", "3");
        assert_eq!(old.map(|x| x.qualified_name()), Some("p:a".into()));
        let names: Vec<String> = a.iter().map(Attr::qualified_name).collect();
        assert_eq!(names, ["q:a", "b"]);
        assert_eq!(a.replace_ns(None, None, "c", "4"), None);
        assert_eq!(a.len(), 3);
    }
}
