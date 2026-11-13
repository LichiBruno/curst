//! the TOML files as written on disk
//!
//! these types are the file format contract: fields may be added with a
//! default, but never renamed or removed, so older files keep loading

use serde::{Deserialize, Serialize};

use crate::model::{Auth, Body, EnvId, KeyValue, Method, Request, Variable};
use crate::vars::VarName;

#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_true(value: &bool) -> bool {
    *value
}

const fn yes() -> bool {
    true
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct PairFile {
    name: String,

    #[serde(default)]
    value: String,

    #[serde(default = "yes", skip_serializing_if = "is_true")]
    enabled: bool,
}

impl From<&KeyValue> for PairFile {
    fn from(pair: &KeyValue) -> Self {
        Self {
            name: pair.name.clone(),
            value: pair.value.clone(),
            enabled: pair.enabled,
        }
    }
}

impl From<PairFile> for KeyValue {
    fn from(pair: PairFile) -> Self {
        Self {
            name: pair.name,
            value: pair.value,
            enabled: pair.enabled,
        }
    }
}

fn pairs(pairs: &[KeyValue]) -> Vec<PairFile> {
    pairs.iter().map(PairFile::from).collect()
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub(crate) enum BodyFile {
    Json {
        text: String,
    },
    Text {
        content_type: String,
        text: String,
    },
    Form {
        #[serde(default)]
        fields: Vec<PairFile>,
    },
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub(crate) enum AuthFile {
    Basic {
        #[serde(default)]
        username: String,

        #[serde(default)]
        password: String,
    },
    Bearer {
        #[serde(default)]
        token: String,
    },
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct RequestFile {
    pub(crate) name: String,

    #[serde(default)]
    method: String,

    #[serde(default)]
    url: String,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    params: Vec<PairFile>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    headers: Vec<PairFile>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    body: Option<BodyFile>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    auth: Option<AuthFile>,
}

impl RequestFile {
    pub(crate) fn new(name: &str, request: &Request) -> Self {
        let body = match &request.body {
            Body::None => None,
            Body::Json(text) => Some(BodyFile::Json { text: text.clone() }),
            Body::Text { content_type, text } => Some(BodyFile::Text {
                content_type: content_type.clone(),
                text: text.clone(),
            }),
            Body::Form(fields) => Some(BodyFile::Form {
                fields: pairs(fields),
            }),
        };
        let auth = match &request.auth {
            Auth::None => None,
            Auth::Basic { username, password } => Some(AuthFile::Basic {
                username: username.clone(),
                password: password.clone(),
            }),
            Auth::Bearer { token } => Some(AuthFile::Bearer {
                token: token.clone(),
            }),
        };

        Self {
            name: name.to_owned(),
            method: request.method.as_str().to_owned(),
            url: request.url.clone(),
            params: pairs(&request.params),
            headers: pairs(&request.headers),
            body,
            auth,
        }
    }

    /// the request, or why the file does not describe one
    pub(crate) fn into_request(self) -> Result<(String, Request), String> {
        let method = if self.method.is_empty() {
            Method::default()
        } else {
            self.method.parse().map_err(|e| format!("{e}"))?
        };
        let body = match self.body {
            None => Body::None,
            Some(BodyFile::Json { text }) => Body::Json(text),
            Some(BodyFile::Text { content_type, text }) => Body::Text { content_type, text },
            Some(BodyFile::Form { fields }) => {
                Body::Form(fields.into_iter().map(KeyValue::from).collect())
            }
        };
        let auth = match self.auth {
            None => Auth::None,
            Some(AuthFile::Basic { username, password }) => Auth::Basic { username, password },
            Some(AuthFile::Bearer { token }) => Auth::Bearer { token },
        };
        let request = Request {
            method,
            url: self.url,
            params: self.params.into_iter().map(KeyValue::from).collect(),
            headers: self.headers.into_iter().map(KeyValue::from).collect(),
            body,
            auth,
        };

        Ok((self.name, request))
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct VariableFile {
    name: String,

    #[serde(default)]
    value: String,

    #[serde(default = "yes", skip_serializing_if = "is_true")]
    enabled: bool,
}

pub(crate) fn variables_to_file(variables: &[Variable]) -> Vec<VariableFile> {
    variables
        .iter()
        .map(|v| VariableFile {
            name: v.name.as_str().to_owned(),
            value: v.value.clone(),
            enabled: v.enabled,
        })
        .collect()
}

pub(crate) fn variables_from_file(variables: Vec<VariableFile>) -> Result<Vec<Variable>, String> {
    variables
        .into_iter()
        .map(|v| {
            let name = VarName::new(&v.name)
                .ok_or_else(|| format!("invalid variable name {:?}", v.name))?;

            Ok(Variable {
                name,
                value: v.value,
                enabled: v.enabled,
            })
        })
        .collect()
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct CollectionFile {
    pub(crate) name: String,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) order: Vec<String>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) variables: Vec<VariableFile>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub(crate) struct FolderFile {
    pub(crate) name: String,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) order: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct EnvironmentFile {
    pub(crate) id: String,
    pub(crate) name: String,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) variables: Vec<VariableFile>,
}

impl EnvironmentFile {
    pub(crate) fn env_id(&self) -> Result<EnvId, String> {
        EnvId::new(&self.id).ok_or_else(|| format!("invalid environment id {:?}", self.id))
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub(crate) struct GlobalsFile {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) variables: Vec<VariableFile>,
}

/// the TOML text of any file, with long strings written over several lines
pub(crate) fn to_toml(value: &impl Serialize) -> String {
    toml::to_string_pretty(value).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_survives_the_file_format() {
        let mut off = KeyValue::new("x-off", "1");
        off.enabled = false;

        for (body, auth) in [
            (Body::None, Auth::None),
            (
                Body::Json("{\n  \"a\": 1\n}".into()),
                Auth::Bearer {
                    token: "{{token}}".into(),
                },
            ),
            (
                Body::Text {
                    content_type: "application/xml".into(),
                    text: "<a/>".into(),
                },
                Auth::Basic {
                    username: "u".into(),
                    password: "p".into(),
                },
            ),
            (
                Body::Form(vec![KeyValue::new("a", "1"), off.clone()]),
                Auth::None,
            ),
        ] {
            let request = Request {
                method: Method::Patch,
                url: "{{base}}/x".into(),
                params: vec![KeyValue::new("q", "1")],
                headers: vec![off.clone()],
                body,
                auth,
            };
            let text = to_toml(&RequestFile::new("Name", &request));
            let back: RequestFile = toml::from_str(&text).unwrap();

            assert_eq!(back.into_request().unwrap(), ("Name".into(), request));
        }
    }

    #[test]
    fn missing_fields_take_defaults_and_bad_methods_fail() {
        let file: RequestFile = toml::from_str("name = \"n\"").unwrap();

        assert_eq!(
            file.into_request().unwrap(),
            ("n".into(), Request::default())
        );

        let file: RequestFile = toml::from_str("name = \"n\"\nmethod = \"BREW\"").unwrap();

        assert!(file.into_request().unwrap_err().contains("BREW"));
    }

    #[test]
    fn variables_and_environment_ids_are_checked() {
        let vars = vec![Variable::new(VarName::new("a").unwrap(), "1")];

        assert_eq!(variables_from_file(variables_to_file(&vars)).unwrap(), vars);

        let bad: GlobalsFile = toml::from_str("[[variables]]\nname = \"a b\"").unwrap();

        assert!(variables_from_file(bad.variables).is_err());

        let env: EnvironmentFile = toml::from_str("id = \"../x\"\nname = \"Dev\"").unwrap();

        assert!(env.env_id().is_err());
    }
}
