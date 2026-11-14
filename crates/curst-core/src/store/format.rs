//! the TOML files as written on disk
//!
//! these types are the file format contract: fields may be added with a
//! default, but never renamed or removed, so older files keep loading

use serde::{Deserialize, Serialize};

use crate::model::{
    Auth, Body, Capture, EnvId, Extract, KeyPlace, KeyValue, Method, Request, Source, SpecOrigin,
    Step, Variable, Workflow,
};
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
    None,
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
    #[serde(rename = "api_key")]
    ApiKey {
        name: String,

        #[serde(default)]
        value: String,

        /// `header`, or `query`
        #[serde(default, rename = "in", skip_serializing_if = "String::is_empty")]
        place: String,
    },
}

impl AuthFile {
    /// the file form of `auth`; inheriting is written as nothing, as a
    /// file from before auth was inherited means the same
    pub(crate) fn of(auth: &Auth) -> Option<Self> {
        match auth {
            Auth::Inherit => None,
            Auth::None => Some(Self::None),
            Auth::Basic { username, password } => Some(Self::Basic {
                username: username.clone(),
                password: password.clone(),
            }),
            Auth::Bearer { token } => Some(Self::Bearer {
                token: token.clone(),
            }),
            Auth::ApiKey { name, value, place } => Some(Self::ApiKey {
                name: name.clone(),
                value: value.clone(),
                place: match place {
                    KeyPlace::Header => String::new(),
                    KeyPlace::Query => "query".into(),
                },
            }),
        }
    }

    pub(crate) fn into_auth(file: Option<Self>) -> Auth {
        match file {
            None => Auth::Inherit,
            Some(Self::None) => Auth::None,
            Some(Self::Basic { username, password }) => Auth::Basic { username, password },
            Some(Self::Bearer { token }) => Auth::Bearer { token },
            Some(Self::ApiKey { name, value, place }) => Auth::ApiKey {
                name,
                value,
                place: if place.eq_ignore_ascii_case("query") {
                    KeyPlace::Query
                } else {
                    KeyPlace::Header
                },
            },
        }
    }
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

    /// for a request imported from a spec, where in it it came from
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) spec: Option<SpecFile>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct SpecFile {
    operation: String,
    fingerprint: String,
}

impl From<&SpecOrigin> for SpecFile {
    fn from(spec: &SpecOrigin) -> Self {
        Self {
            operation: spec.operation.clone(),
            fingerprint: spec.fingerprint.clone(),
        }
    }
}

impl From<SpecFile> for SpecOrigin {
    fn from(spec: SpecFile) -> Self {
        Self {
            operation: spec.operation,
            fingerprint: spec.fingerprint,
        }
    }
}

/// where a collection's spec is fetched again
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct SourceFile {
    location: String,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    picked: Option<String>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    headers: Vec<PairFile>,
}

impl From<&Source> for SourceFile {
    fn from(source: &Source) -> Self {
        Self {
            location: source.location.clone(),
            picked: source.picked.clone(),
            headers: pairs(&source.headers),
        }
    }
}

impl From<SourceFile> for Source {
    fn from(file: SourceFile) -> Self {
        Self {
            location: file.location,
            headers: file.headers.into_iter().map(KeyValue::from).collect(),
            picked: file.picked,
        }
    }
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
        let auth = AuthFile::of(&request.auth);

        Self {
            name: name.to_owned(),
            method: request.method.as_str().to_owned(),
            url: request.url.clone(),
            params: pairs(&request.params),
            headers: pairs(&request.headers),
            body,
            auth,
            spec: None,
        }
    }

    /// the file of a request made from a spec
    pub(crate) fn with_spec(mut self, spec: Option<&SpecOrigin>) -> Self {
        self.spec = spec.map(SpecFile::from);

        self
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
        let auth = AuthFile::into_auth(self.auth);
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

#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_false(value: &bool) -> bool {
    !*value
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct VariableFile {
    name: String,

    /// absent for a secret, whose value lives in the secret store
    #[serde(default, skip_serializing_if = "Option::is_none")]
    value: Option<String>,

    #[serde(default = "yes", skip_serializing_if = "is_true")]
    enabled: bool,

    #[serde(default, skip_serializing_if = "is_false")]
    secret: bool,
}

/// the variables as written to a file; a secret's value is never included,
/// whatever the variable holds in memory
pub(crate) fn variables_to_file(variables: &[Variable]) -> Vec<VariableFile> {
    variables
        .iter()
        .map(|v| VariableFile {
            name: v.name.as_str().to_owned(),
            value: (!v.secret).then(|| v.value.clone()),
            enabled: v.enabled,
            secret: v.secret,
        })
        .collect()
}

/// variables read from a file, and a warning for each secret whose value
/// was written into it by hand
pub(crate) struct ReadVariables {
    pub(crate) variables: Vec<Variable>,
    pub(crate) warnings: Vec<String>,
}

pub(crate) fn variables_from_file(variables: Vec<VariableFile>) -> Result<ReadVariables, String> {
    let mut read = ReadVariables {
        variables: Vec::new(),
        warnings: Vec::new(),
    };

    for v in variables {
        let name =
            VarName::new(&v.name).ok_or_else(|| format!("invalid variable name {:?}", v.name))?;

        // the written value is never used, and the next save leaves it out
        if v.secret && v.value.is_some() {
            read.warnings.push(format!(
                "{name} is a secret, but its value is written in the file: that value is \
                 not used; remove it and set the secret in curst"
            ));
        }

        let value = if v.secret {
            String::new()
        } else {
            v.value.unwrap_or_default()
        };

        read.variables.push(Variable {
            name,
            value,
            enabled: v.enabled,
            secret: v.secret,
        });
    }

    Ok(read)
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct CollectionFile {
    pub(crate) name: String,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) order: Vec<String>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) variables: Vec<VariableFile>,

    /// for a collection imported from a spec, where to fetch it again
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) source: Option<SourceFile>,

    /// the auth its requests inherit
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) auth: Option<AuthFile>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) workflows: Vec<WorkflowFile>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct WorkflowFile {
    pub(crate) name: String,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) steps: Vec<StepFile>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct StepFile {
    /// the request as its path of slugs, or of names in a bundle
    pub(crate) request: Vec<String>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) keep: Vec<CaptureFile>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct CaptureFile {
    variable: String,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    json: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    header: Option<String>,
}

impl WorkflowFile {
    pub(crate) fn of(workflow: &Workflow) -> Self {
        Self {
            name: workflow.name.clone(),
            steps: workflow
                .steps
                .iter()
                .map(|step| StepFile {
                    request: step.request.clone(),
                    keep: step
                        .keep
                        .iter()
                        .map(|capture| {
                            let (json, header) = match &capture.from {
                                Extract::Json(path) => (Some(path.clone()), None),
                                Extract::Header(name) => (None, Some(name.clone())),
                            };

                            CaptureFile {
                                variable: capture.variable.as_str().to_owned(),
                                json,
                                header,
                            }
                        })
                        .collect(),
                })
                .collect(),
        }
    }

    /// the workflow, or why the file does not describe one
    pub(crate) fn into_workflow(self) -> Result<Workflow, String> {
        let mut steps = Vec::new();

        for step in self.steps {
            let mut keep = Vec::new();

            for capture in step.keep {
                let variable = VarName::new(&capture.variable).ok_or_else(|| {
                    format!(
                        "the workflow {}: {:?} is not a variable name",
                        self.name, capture.variable
                    )
                })?;
                let from = match (capture.json, capture.header) {
                    (Some(path), None) => Extract::Json(path),
                    (None, Some(name)) => Extract::Header(name),
                    _ => {
                        return Err(format!(
                            "the workflow {}: {variable} needs either json or header",
                            self.name
                        ));
                    }
                };

                keep.push(Capture { variable, from });
            }

            steps.push(Step {
                request: step.request,
                keep,
            });
        }

        Ok(Workflow {
            name: self.name,
            steps,
        })
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub(crate) struct FolderFile {
    pub(crate) name: String,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) order: Vec<String>,

    /// the auth its requests inherit, when it sets one
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) auth: Option<AuthFile>,
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
    fn a_secret_value_never_reaches_the_file() {
        let mut token = Variable::secret(VarName::new("token").unwrap());

        token.value = "s3cr3t".into();

        let file = EnvironmentFile {
            id: "e1".into(),
            name: "Dev".into(),
            variables: variables_to_file(&[
                Variable::new(VarName::new("host").unwrap(), "h"),
                token,
            ]),
        };
        let text = to_toml(&file);

        assert_eq!(
            text,
            "id = \"e1\"\nname = \"Dev\"\n\n[[variables]]\nname = \"host\"\nvalue = \"h\"\n\n[[variables]]\nname = \"token\"\nsecret = true\n"
        );

        let collection = to_toml(&CollectionFile {
            name: "c".into(),
            order: vec![],
            variables: file.variables,
            source: None,
            auth: None,
            workflows: Vec::new(),
        });
        let globals = to_toml(&GlobalsFile {
            variables: variables_to_file(&[Variable {
                value: "s3cr3t".into(),
                ..Variable::secret(VarName::new("g").unwrap())
            }]),
        });

        assert!(!collection.contains("s3cr3t"));
        assert!(!globals.contains("s3cr3t"));
        assert!(globals.contains("secret = true"));
    }

    #[test]
    fn a_hand_written_value_beside_secret_is_ignored() {
        let file: GlobalsFile =
            toml::from_str("[[variables]]\nname = \"t\"\nvalue = \"oops\"\nsecret = true\n")
                .unwrap();
        let read = variables_from_file(file.variables).unwrap();

        assert_eq!(
            read.variables,
            [Variable::secret(VarName::new("t").unwrap())]
        );
        assert_eq!(read.warnings.len(), 1);
        assert!(read.warnings[0].starts_with("t is a secret, but its value is written"));
    }

    #[test]
    fn variables_and_environment_ids_are_checked() {
        let vars = vec![Variable::new(VarName::new("a").unwrap(), "1")];

        let read = variables_from_file(variables_to_file(&vars)).unwrap();

        assert_eq!(read.variables, vars);
        assert_eq!(read.warnings, Vec::<String>::new());

        let bad: GlobalsFile = toml::from_str("[[variables]]\nname = \"a b\"").unwrap();

        assert!(variables_from_file(bad.variables).is_err());

        let env: EnvironmentFile = toml::from_str("id = \"../x\"\nname = \"Dev\"").unwrap();

        assert!(env.env_id().is_err());
    }
}
