//! Filesystem paths cross as strings.
//!
//! The outbound conversion is infallible, so a non-UTF-8 path crosses
//! lossily (invalid sequences become U+FFFD, `Path::to_string_lossy`
//! semantics). Hosts that must round-trip raw non-UTF-8 paths should carry
//! them as `Vec<u8>`/bytes instead.

use std::path::{Path, PathBuf};

use crate::bridge::{FromScript, ScriptConvertError, ScriptValue};
use crate::haphe_type::HapheType;
use crate::types::TypeDescriptor;

impl HapheType for PathBuf {
    const DESCRIPTOR: TypeDescriptor<'static> = TypeDescriptor::String;
}

impl HapheType for Path {
    const DESCRIPTOR: TypeDescriptor<'static> = TypeDescriptor::String;
}

impl From<PathBuf> for ScriptValue {
    fn from(path: PathBuf) -> Self {
        match path.into_os_string().into_string() {
            Ok(s) => Self::String(s),
            Err(os) => Self::String(os.to_string_lossy().into_owned()),
        }
    }
}

impl From<&Path> for ScriptValue {
    fn from(path: &Path) -> Self {
        Self::String(path.to_string_lossy().into_owned())
    }
}

impl FromScript for PathBuf {
    fn from_script(v: ScriptValue) -> Result<Self, ScriptConvertError> {
        match v {
            ScriptValue::String(s) => Ok(PathBuf::from(s)),
            other => Err(ScriptConvertError {
                expected: "path string",
                got: other.variant_name(),
            }),
        }
    }
}
