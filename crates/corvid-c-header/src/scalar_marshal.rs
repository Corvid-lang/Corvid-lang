use corvid_types::Type;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScalarAbiType {
    Int,
    Float,
    Bool,
    String,
    Nothing,
}

impl ScalarAbiType {
    pub fn from_type(ty: &Type) -> Option<Self> {
        match ty {
            Type::Int => Some(Self::Int),
            Type::Float => Some(Self::Float),
            Type::Bool => Some(Self::Bool),
            Type::String => Some(Self::String),
            Type::Nothing => Some(Self::Nothing),
            _ => None,
        }
    }

    pub fn c_param_type(self) -> &'static str {
        match self {
            Self::Int => "int64_t",
            Self::Float => "double",
            Self::Bool => "bool",
            Self::String => "const char*",
            Self::Nothing => "void",
        }
    }

    pub fn c_return_type(self) -> &'static str {
        match self {
            Self::Int => "int64_t",
            Self::Float => "double",
            Self::Bool => "bool",
            Self::String => "const char*",
            Self::Nothing => "void",
        }
    }
}
