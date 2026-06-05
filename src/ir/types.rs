use std::fmt::{self, Display, Formatter};

#[derive(Clone, PartialEq, Debug)]
pub enum Dtype {
    Void,
    I1,
    I32,
    Struct {
        type_name: String,
    },
    Pointer {
        pointee: Box<Dtype>,
    },
    Array {
        element: Box<Dtype>,
        length: Option<usize>,
    },
}

impl Dtype {
    pub fn ptr_to(pointee: Self) -> Self {
        Self::Pointer {
            pointee: Box::new(pointee),
        }
    }

    pub fn array_of(elem: Self, len: usize) -> Self {
        Self::Array {
            element: Box::new(elem),
            length: Some(len),
        }
    }

    pub fn struct_type_name(&self) -> Option<&String> {
        match self {
            Dtype::Struct { type_name } => Some(type_name),
            Dtype::Pointer { pointee } => pointee.struct_type_name(),
            Dtype::Array { element, .. } => element.struct_type_name(),
            _ => None,
        }
    }
}

impl Display for Dtype {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Dtype::I1 => write!(f, "i1"),
            Dtype::I32 => write!(f, "i32"),
            Dtype::Void => write!(f, "void"),
            Dtype::Struct { type_name } => write!(f, "%{type_name}"),
            Dtype::Pointer { .. } => write!(f, "ptr"),
            Dtype::Array {
                element,
                length: Some(length),
            } => write!(f, "[{length} x {}]", element.as_ref()),
            Dtype::Array {
                element,
                length: None,
            } => write!(f, "{}", element.as_ref()),
        }
    }
}

pub struct StructMember {
    /// Zero-based field index within the enclosing struct.  Emitted as the
    /// second GEP index when accessing this member.
    pub index: usize,
    pub dtype: Dtype,
}

pub struct StructType {
    pub elements: Vec<(String, StructMember)>,
}

#[derive(Clone, PartialEq)]
pub struct FunctionType {
    pub return_dtype: Dtype,
    pub arguments: Vec<(String, Dtype)>,
}

