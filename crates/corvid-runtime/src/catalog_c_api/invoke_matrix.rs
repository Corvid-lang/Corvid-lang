//! Scalar ABI invocation matrix for catalog host dispatch.

use crate::catalog::{ScalarAbiType, ScalarInvocation, ScalarInvoker, ScalarReturnType};
use crate::errors::RuntimeError;
use crate::observation_handles;
use std::ffi::{c_char, CStr, CString};
use std::sync::Arc;

pub(crate) fn build_scalar_invoker(
    symbol: &str,
    params: &[ScalarAbiType],
    ret: ScalarReturnType,
) -> Result<ScalarInvoker, RuntimeError> {
    unsafe {
        let address = super::resolve_current_library_symbol(symbol)? as usize;
        if address == 0 {
            return Err(RuntimeError::Other(format!(
                "symbol `{symbol}` resolved to null"
            )));
        }
        match params {
            [] => build_invoker0(symbol.to_string(), address, ret),
            [a0] => build_invoker1(symbol.to_string(), address, *a0, ret),
            [a0, a1] => build_invoker2(symbol.to_string(), address, *a0, *a1, ret),
            _ => Err(RuntimeError::Other(format!(
                "catalog host dispatch currently supports up to two scalar parameters; `{symbol}` has {}",
                params.len()
            ))),
        }
    }
}

unsafe fn build_invoker0(
    symbol: String,
    address: usize,
    ret: ScalarReturnType,
) -> Result<ScalarInvoker, RuntimeError> {
    Ok(Arc::new(move |args| {
        if !args.is_empty() {
            return Err(RuntimeError::Marshal(format!(
                "agent `{symbol}` expected 0 args, got {}",
                args.len()
            )));
        }
        invoke0(&symbol, address, ret)
    }))
}

unsafe fn build_invoker1(
    symbol: String,
    address: usize,
    a0: ScalarAbiType,
    ret: ScalarReturnType,
) -> Result<ScalarInvoker, RuntimeError> {
    Ok(Arc::new(move |args| {
        if args.len() != 1 {
            return Err(RuntimeError::Marshal(format!(
                "agent `{symbol}` expected 1 arg, got {}",
                args.len()
            )));
        }
        invoke1(&symbol, address, a0, ret, &args[0])
    }))
}

unsafe fn build_invoker2(
    symbol: String,
    address: usize,
    a0: ScalarAbiType,
    a1: ScalarAbiType,
    ret: ScalarReturnType,
) -> Result<ScalarInvoker, RuntimeError> {
    Ok(Arc::new(move |args| {
        if args.len() != 2 {
            return Err(RuntimeError::Marshal(format!(
                "agent `{symbol}` expected 2 args, got {}",
                args.len()
            )));
        }
        invoke2(&symbol, address, a0, a1, ret, &args[0], &args[1])
    }))
}

unsafe fn invoke0(
    symbol: &str,
    address: usize,
    ret: ScalarReturnType,
) -> Result<ScalarInvocation, RuntimeError> {
    let mut observation_handle = observation_handles::NULL_OBSERVATION_HANDLE;
    match ret {
        ScalarReturnType::Int => {
            let func: unsafe extern "C" fn(*mut u64) -> i64 = std::mem::transmute(address);
            Ok(ScalarInvocation {
                result: serde_json::Value::from(func(&mut observation_handle)),
                observation_handle,
            })
        }
        ScalarReturnType::Float => {
            let func: unsafe extern "C" fn(*mut u64) -> f64 = std::mem::transmute(address);
            Ok(ScalarInvocation {
                result: float_json(symbol, func(&mut observation_handle))?,
                observation_handle,
            })
        }
        ScalarReturnType::Bool => {
            let func: unsafe extern "C" fn(*mut u64) -> bool = std::mem::transmute(address);
            Ok(ScalarInvocation {
                result: serde_json::Value::Bool(func(&mut observation_handle)),
                observation_handle,
            })
        }
        ScalarReturnType::String => {
            let func: unsafe extern "C" fn(*mut u64) -> *const c_char =
                std::mem::transmute(address);
            Ok(ScalarInvocation {
                result: string_json(symbol, func(&mut observation_handle))?,
                observation_handle,
            })
        }
        ScalarReturnType::Nothing => {
            let func: unsafe extern "C" fn(*mut u64) = std::mem::transmute(address);
            func(&mut observation_handle);
            Ok(ScalarInvocation {
                result: serde_json::Value::Null,
                observation_handle,
            })
        }
    }
}

unsafe fn invoke1(
    symbol: &str,
    address: usize,
    a0: ScalarAbiType,
    ret: ScalarReturnType,
    arg0: &serde_json::Value,
) -> Result<ScalarInvocation, RuntimeError> {
    match a0 {
        ScalarAbiType::Int => invoke1_int(symbol, address, ret, parse_i64_arg(arg0, symbol, 0)?),
        ScalarAbiType::Float => {
            invoke1_float(symbol, address, ret, parse_f64_arg(arg0, symbol, 0)?)
        }
        ScalarAbiType::Bool => invoke1_bool(symbol, address, ret, parse_bool_arg(arg0, symbol, 0)?),
        ScalarAbiType::String => {
            let arg0 = parse_string_arg(arg0, symbol, 0)?;
            invoke1_string(symbol, address, ret, arg0.as_ptr())
        }
    }
}

unsafe fn invoke2(
    symbol: &str,
    address: usize,
    a0: ScalarAbiType,
    a1: ScalarAbiType,
    ret: ScalarReturnType,
    arg0: &serde_json::Value,
    arg1: &serde_json::Value,
) -> Result<ScalarInvocation, RuntimeError> {
    match a0 {
        ScalarAbiType::Int => {
            let arg0 = parse_i64_arg(arg0, symbol, 0)?;
            invoke2_after_int(symbol, address, a1, ret, arg0, arg1)
        }
        ScalarAbiType::Float => {
            let arg0 = parse_f64_arg(arg0, symbol, 0)?;
            invoke2_after_float(symbol, address, a1, ret, arg0, arg1)
        }
        ScalarAbiType::Bool => {
            let arg0 = parse_bool_arg(arg0, symbol, 0)?;
            invoke2_after_bool(symbol, address, a1, ret, arg0, arg1)
        }
        ScalarAbiType::String => {
            let arg0 = parse_string_arg(arg0, symbol, 0)?;
            invoke2_after_string(symbol, address, a1, ret, arg0.as_ptr(), arg1)
        }
    }
}

macro_rules! impl_invoke1 {
    ($name:ident, $arg_ty:ty) => {
        unsafe fn $name(
            symbol: &str,
            address: usize,
            ret: ScalarReturnType,
            arg0: $arg_ty,
        ) -> Result<ScalarInvocation, RuntimeError> {
            let mut observation_handle = observation_handles::NULL_OBSERVATION_HANDLE;
            // Why: exported `pub extern "C"` wrappers append a hidden
            // observation-handle out-pointer after the user-visible
            // arguments. Generic host dispatch must pass that pointer
            // too; skipping it "worked" on Linux by luck and crashed
            // on Windows with a misaligned pointer in
            // `corvid_finish_direct_observation`.
            match ret {
                ScalarReturnType::Int => {
                    let func: unsafe extern "C" fn($arg_ty, *mut u64) -> i64 =
                        std::mem::transmute(address);
                    Ok(ScalarInvocation {
                        result: serde_json::Value::from(func(arg0, &mut observation_handle)),
                        observation_handle,
                    })
                }
                ScalarReturnType::Float => {
                    let func: unsafe extern "C" fn($arg_ty, *mut u64) -> f64 =
                        std::mem::transmute(address);
                    Ok(ScalarInvocation {
                        result: float_json(symbol, func(arg0, &mut observation_handle))?,
                        observation_handle,
                    })
                }
                ScalarReturnType::Bool => {
                    let func: unsafe extern "C" fn($arg_ty, *mut u64) -> bool =
                        std::mem::transmute(address);
                    Ok(ScalarInvocation {
                        result: serde_json::Value::Bool(func(arg0, &mut observation_handle)),
                        observation_handle,
                    })
                }
                ScalarReturnType::String => {
                    let func: unsafe extern "C" fn($arg_ty, *mut u64) -> *const c_char =
                        std::mem::transmute(address);
                    Ok(ScalarInvocation {
                        result: string_json(symbol, func(arg0, &mut observation_handle))?,
                        observation_handle,
                    })
                }
                ScalarReturnType::Nothing => {
                    let func: unsafe extern "C" fn($arg_ty, *mut u64) =
                        std::mem::transmute(address);
                    func(arg0, &mut observation_handle);
                    Ok(ScalarInvocation {
                        result: serde_json::Value::Null,
                        observation_handle,
                    })
                }
            }
        }
    };
}

impl_invoke1!(invoke1_int, i64);
impl_invoke1!(invoke1_float, f64);
impl_invoke1!(invoke1_bool, bool);
impl_invoke1!(invoke1_string, *const c_char);

macro_rules! impl_invoke2_matrix {
    ($name:ident, $arg0_ty:ty, $arg1_ty:ty) => {
        unsafe fn $name(
            symbol: &str,
            address: usize,
            ret: ScalarReturnType,
            arg0: $arg0_ty,
            arg1: $arg1_ty,
        ) -> Result<ScalarInvocation, RuntimeError> {
            let mut observation_handle = observation_handles::NULL_OBSERVATION_HANDLE;
            match ret {
                ScalarReturnType::Int => {
                    let func: unsafe extern "C" fn($arg0_ty, $arg1_ty, *mut u64) -> i64 =
                        std::mem::transmute(address);
                    Ok(ScalarInvocation {
                        result: serde_json::Value::from(func(arg0, arg1, &mut observation_handle)),
                        observation_handle,
                    })
                }
                ScalarReturnType::Float => {
                    let func: unsafe extern "C" fn($arg0_ty, $arg1_ty, *mut u64) -> f64 =
                        std::mem::transmute(address);
                    Ok(ScalarInvocation {
                        result: float_json(symbol, func(arg0, arg1, &mut observation_handle))?,
                        observation_handle,
                    })
                }
                ScalarReturnType::Bool => {
                    let func: unsafe extern "C" fn($arg0_ty, $arg1_ty, *mut u64) -> bool =
                        std::mem::transmute(address);
                    Ok(ScalarInvocation {
                        result: serde_json::Value::Bool(func(arg0, arg1, &mut observation_handle)),
                        observation_handle,
                    })
                }
                ScalarReturnType::String => {
                    let func: unsafe extern "C" fn($arg0_ty, $arg1_ty, *mut u64) -> *const c_char =
                        std::mem::transmute(address);
                    Ok(ScalarInvocation {
                        result: string_json(symbol, func(arg0, arg1, &mut observation_handle))?,
                        observation_handle,
                    })
                }
                ScalarReturnType::Nothing => {
                    let func: unsafe extern "C" fn($arg0_ty, $arg1_ty, *mut u64) =
                        std::mem::transmute(address);
                    func(arg0, arg1, &mut observation_handle);
                    Ok(ScalarInvocation {
                        result: serde_json::Value::Null,
                        observation_handle,
                    })
                }
            }
        }
    };
}

impl_invoke2_matrix!(invoke2_i64_i64, i64, i64);
impl_invoke2_matrix!(invoke2_i64_f64, i64, f64);
impl_invoke2_matrix!(invoke2_i64_bool, i64, bool);
impl_invoke2_matrix!(invoke2_i64_string, i64, *const c_char);
impl_invoke2_matrix!(invoke2_f64_i64, f64, i64);
impl_invoke2_matrix!(invoke2_f64_f64, f64, f64);
impl_invoke2_matrix!(invoke2_f64_bool, f64, bool);
impl_invoke2_matrix!(invoke2_f64_string, f64, *const c_char);
impl_invoke2_matrix!(invoke2_bool_i64, bool, i64);
impl_invoke2_matrix!(invoke2_bool_f64, bool, f64);
impl_invoke2_matrix!(invoke2_bool_bool, bool, bool);
impl_invoke2_matrix!(invoke2_bool_string, bool, *const c_char);
impl_invoke2_matrix!(invoke2_string_i64, *const c_char, i64);
impl_invoke2_matrix!(invoke2_string_f64, *const c_char, f64);
impl_invoke2_matrix!(invoke2_string_bool, *const c_char, bool);
impl_invoke2_matrix!(invoke2_string_string, *const c_char, *const c_char);

unsafe fn invoke2_after_int(
    symbol: &str,
    address: usize,
    a1: ScalarAbiType,
    ret: ScalarReturnType,
    arg0: i64,
    arg1: &serde_json::Value,
) -> Result<ScalarInvocation, RuntimeError> {
    match a1 {
        ScalarAbiType::Int => {
            invoke2_i64_i64(symbol, address, ret, arg0, parse_i64_arg(arg1, symbol, 1)?)
        }
        ScalarAbiType::Float => {
            invoke2_i64_f64(symbol, address, ret, arg0, parse_f64_arg(arg1, symbol, 1)?)
        }
        ScalarAbiType::Bool => {
            invoke2_i64_bool(symbol, address, ret, arg0, parse_bool_arg(arg1, symbol, 1)?)
        }
        ScalarAbiType::String => {
            let arg1 = parse_string_arg(arg1, symbol, 1)?;
            invoke2_i64_string(symbol, address, ret, arg0, arg1.as_ptr())
        }
    }
}

unsafe fn invoke2_after_float(
    symbol: &str,
    address: usize,
    a1: ScalarAbiType,
    ret: ScalarReturnType,
    arg0: f64,
    arg1: &serde_json::Value,
) -> Result<ScalarInvocation, RuntimeError> {
    match a1 {
        ScalarAbiType::Int => {
            invoke2_f64_i64(symbol, address, ret, arg0, parse_i64_arg(arg1, symbol, 1)?)
        }
        ScalarAbiType::Float => {
            invoke2_f64_f64(symbol, address, ret, arg0, parse_f64_arg(arg1, symbol, 1)?)
        }
        ScalarAbiType::Bool => {
            invoke2_f64_bool(symbol, address, ret, arg0, parse_bool_arg(arg1, symbol, 1)?)
        }
        ScalarAbiType::String => {
            let arg1 = parse_string_arg(arg1, symbol, 1)?;
            invoke2_f64_string(symbol, address, ret, arg0, arg1.as_ptr())
        }
    }
}

unsafe fn invoke2_after_bool(
    symbol: &str,
    address: usize,
    a1: ScalarAbiType,
    ret: ScalarReturnType,
    arg0: bool,
    arg1: &serde_json::Value,
) -> Result<ScalarInvocation, RuntimeError> {
    match a1 {
        ScalarAbiType::Int => {
            invoke2_bool_i64(symbol, address, ret, arg0, parse_i64_arg(arg1, symbol, 1)?)
        }
        ScalarAbiType::Float => {
            invoke2_bool_f64(symbol, address, ret, arg0, parse_f64_arg(arg1, symbol, 1)?)
        }
        ScalarAbiType::Bool => {
            invoke2_bool_bool(symbol, address, ret, arg0, parse_bool_arg(arg1, symbol, 1)?)
        }
        ScalarAbiType::String => {
            let arg1 = parse_string_arg(arg1, symbol, 1)?;
            invoke2_bool_string(symbol, address, ret, arg0, arg1.as_ptr())
        }
    }
}

unsafe fn invoke2_after_string(
    symbol: &str,
    address: usize,
    a1: ScalarAbiType,
    ret: ScalarReturnType,
    arg0: *const c_char,
    arg1: &serde_json::Value,
) -> Result<ScalarInvocation, RuntimeError> {
    match a1 {
        ScalarAbiType::Int => {
            invoke2_string_i64(symbol, address, ret, arg0, parse_i64_arg(arg1, symbol, 1)?)
        }
        ScalarAbiType::Float => {
            invoke2_string_f64(symbol, address, ret, arg0, parse_f64_arg(arg1, symbol, 1)?)
        }
        ScalarAbiType::Bool => {
            invoke2_string_bool(symbol, address, ret, arg0, parse_bool_arg(arg1, symbol, 1)?)
        }
        ScalarAbiType::String => {
            let arg1 = parse_string_arg(arg1, symbol, 1)?;
            invoke2_string_string(symbol, address, ret, arg0, arg1.as_ptr())
        }
    }
}

fn float_json(symbol: &str, value: f64) -> Result<serde_json::Value, RuntimeError> {
    let Some(number) = serde_json::Number::from_f64(value) else {
        return Err(RuntimeError::Marshal(format!(
            "agent `{symbol}` returned non-finite Float {value}"
        )));
    };
    Ok(serde_json::Value::Number(number))
}

unsafe fn string_json(
    symbol: &str,
    value: *const c_char,
) -> Result<serde_json::Value, RuntimeError> {
    if value.is_null() {
        return Err(RuntimeError::Marshal(format!(
            "agent `{symbol}` returned null String pointer"
        )));
    }
    let text = CStr::from_ptr(value)
        .to_str()
        .map_err(|err| {
            RuntimeError::Marshal(format!("agent `{symbol}` returned non-UTF8 String: {err}"))
        })?
        .to_owned();
    crate::ffi_bridge::corvid_free_string(value);
    Ok(serde_json::Value::String(text))
}

fn parse_i64_arg(
    value: &serde_json::Value,
    symbol: &str,
    index: usize,
) -> Result<i64, RuntimeError> {
    value.as_i64().ok_or_else(|| {
        RuntimeError::Marshal(format!(
            "agent `{symbol}` argument {} expected Int",
            index + 1
        ))
    })
}

fn parse_f64_arg(
    value: &serde_json::Value,
    symbol: &str,
    index: usize,
) -> Result<f64, RuntimeError> {
    value.as_f64().ok_or_else(|| {
        RuntimeError::Marshal(format!(
            "agent `{symbol}` argument {} expected Float",
            index + 1
        ))
    })
}

fn parse_bool_arg(
    value: &serde_json::Value,
    symbol: &str,
    index: usize,
) -> Result<bool, RuntimeError> {
    value.as_bool().ok_or_else(|| {
        RuntimeError::Marshal(format!(
            "agent `{symbol}` argument {} expected Bool",
            index + 1
        ))
    })
}

fn parse_string_arg(
    value: &serde_json::Value,
    symbol: &str,
    index: usize,
) -> Result<CString, RuntimeError> {
    let text = value.as_str().ok_or_else(|| {
        RuntimeError::Marshal(format!(
            "agent `{symbol}` argument {} expected String",
            index + 1
        ))
    })?;
    CString::new(text).map_err(|err| {
        RuntimeError::Marshal(format!(
            "agent `{symbol}` argument {} contained NUL: {err}",
            index + 1
        ))
    })
}
