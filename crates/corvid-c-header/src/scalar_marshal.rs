use corvid_types::Type;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScalarAbiType {
    Int,
    Float,
    Bool,
    String,
    Nothing,
    GroundedInt,
    GroundedFloat,
    GroundedBool,
    GroundedString,
}

impl ScalarAbiType {
    pub fn from_param_type(ty: &Type) -> Option<Self> {
        match ty {
            Type::Int => Some(Self::Int),
            Type::Float => Some(Self::Float),
            Type::Bool => Some(Self::Bool),
            Type::String => Some(Self::String),
            Type::Nothing => None,
            _ => None,
        }
    }

    pub fn from_return_type(ty: &Type) -> Option<Self> {
        match ty {
            Type::Int => Some(Self::Int),
            Type::Float => Some(Self::Float),
            Type::Bool => Some(Self::Bool),
            Type::String => Some(Self::String),
            Type::Nothing => Some(Self::Nothing),
            Type::Grounded(inner) => match inner.as_ref() {
                Type::Int => Some(Self::GroundedInt),
                Type::Float => Some(Self::GroundedFloat),
                Type::Bool => Some(Self::GroundedBool),
                Type::String => Some(Self::GroundedString),
                _ => None,
            },
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
            Self::GroundedInt | Self::GroundedFloat | Self::GroundedBool | Self::GroundedString => {
                unreachable!("grounded values are return-only at the C ABI boundary")
            }
        }
    }

    pub fn c_return_type(self) -> &'static str {
        match self {
            Self::Int => "int64_t",
            Self::Float => "double",
            Self::Bool => "bool",
            Self::String => "const char*",
            Self::Nothing => "void",
            Self::GroundedInt => "int64_t",
            Self::GroundedFloat => "double",
            Self::GroundedBool => "bool",
            Self::GroundedString => "const char*",
        }
    }

    pub fn is_grounded_return(self) -> bool {
        matches!(
            self,
            Self::GroundedInt | Self::GroundedFloat | Self::GroundedBool | Self::GroundedString
        )
    }
}
