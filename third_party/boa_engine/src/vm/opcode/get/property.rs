use crate::{
    object::{internal_methods::InternalMethodContext, shape::slot::SlotAttributes},
    property::PropertyKey,
    vm::{opcode::Operation, CompletionType},
    Context, JsNativeError, JsObject, JsResult, JsValue,
};

/// `ToObject` of a property access base. Null and undefined throw V8's message, e.g.
/// "Cannot read properties of undefined (reading 'x')" (`set`: "Cannot set … (setting 'x')").
pub(crate) fn access_base(
    value: &JsValue,
    key: impl FnOnce(&Context) -> Option<String>,
    set: bool,
    context: &mut Context,
) -> JsResult<JsObject> {
    if let Some(object) = value.as_object() {
        return Ok(object.clone());
    }
    if value.is_null_or_undefined() {
        let base = if value.is_null() { "null" } else { "undefined" };
        let (verb, gerund) = if set {
            ("set", "setting")
        } else {
            ("read", "reading")
        };
        let message = match key(context) {
            Some(key) => format!("Cannot {verb} properties of {base} ({gerund} '{key}')"),
            None => format!("Cannot {verb} properties of {base}"),
        };
        if log::log_enabled!(target: "axiom_js::stack", log::Level::Debug) {
            let frame = context.vm.frame();
            if let Some(expr) = frame.code_block().call_site(frame.pc) {
                log::debug!(
                    target: "axiom_js::stack",
                    "{message}: `{}` is {base}",
                    expr.to_std_string_escaped()
                );
            }
        }
        return Err(JsNativeError::typ().with_message(message).into());
    }
    value.to_object(context)
}

/// Text of a property key for error messages, without running user code (`None` for an
/// object key, whose conversion could).
pub(crate) fn key_text(key: &JsValue) -> Option<String> {
    if let Some(s) = key.as_string() {
        return Some(s.to_std_string_escaped());
    }
    (!key.is_object()).then(|| key.display().to_string())
}

fn ic_name(context: &Context, index: usize) -> Option<String> {
    Some(context.vm.frame().code_block().ic[index].name.to_std_string_escaped())
}

/// `GetPropertyByName` implements the Opcode Operation for `Opcode::GetPropertyByName`
///
/// Operation:
///  - Get a property by name from an object an push it on the stack.
#[derive(Debug, Clone, Copy)]
pub(crate) struct GetPropertyByName;

impl GetPropertyByName {
    fn operation(context: &mut Context, index: usize) -> JsResult<CompletionType> {
        let receiver = context.vm.pop();
        let value = context.vm.pop();
        let object = access_base(&value, |c| ic_name(c, index), false, context)?;

        let ic = &context.vm.frame().code_block().ic[index];
        let object_borrowed = object.borrow();
        if let Some((shape, slot)) = ic.match_or_reset(object_borrowed.shape()) {
            let mut result = if slot.attributes.contains(SlotAttributes::PROTOTYPE) {
                let prototype = shape.prototype().expect("prototype should have value");
                let prototype = prototype.borrow();
                prototype.properties().storage[slot.index as usize].clone()
            } else {
                object_borrowed.properties().storage[slot.index as usize].clone()
            };

            drop(object_borrowed);
            if slot.attributes.has_get() && result.is_object() {
                result = result.as_object().expect("should contain getter").call(
                    &receiver,
                    &[],
                    context,
                )?;
            }
            context.vm.push(result);
            return Ok(CompletionType::Normal);
        }

        drop(object_borrowed);

        let key: PropertyKey = ic.name.clone().into();

        let context = &mut InternalMethodContext::new(context);
        let result = object.__get__(&key, receiver, context)?;

        // Cache the property.
        let slot = *context.slot();
        if slot.is_cachable() {
            let ic = &context.vm.frame().code_block.ic[index];
            let object_borrowed = object.borrow();
            let shape = object_borrowed.shape();
            ic.set(shape, slot);
        }

        context.vm.push(result);
        Ok(CompletionType::Normal)
    }
}

impl Operation for GetPropertyByName {
    const NAME: &'static str = "GetPropertyByName";
    const INSTRUCTION: &'static str = "INST - GetPropertyByName";
    const COST: u8 = 4;

    fn execute(context: &mut Context) -> JsResult<CompletionType> {
        let index = context.vm.read::<u8>();
        Self::operation(context, index as usize)
    }

    fn execute_with_u16_operands(context: &mut Context) -> JsResult<CompletionType> {
        let index = context.vm.read::<u16>() as usize;
        Self::operation(context, index)
    }

    fn execute_with_u32_operands(context: &mut Context) -> JsResult<CompletionType> {
        let index = context.vm.read::<u32>();
        Self::operation(context, index as usize)
    }
}

/// `GetPropertyByValue` implements the Opcode Operation for `Opcode::GetPropertyByValue`
///
/// Operation:
///  - Get a property by value from an object an push it on the stack.
#[derive(Debug, Clone, Copy)]
pub(crate) struct GetPropertyByValue;

impl Operation for GetPropertyByValue {
    const NAME: &'static str = "GetPropertyByValue";
    const INSTRUCTION: &'static str = "INST - GetPropertyByValue";
    const COST: u8 = 4;

    fn execute(context: &mut Context) -> JsResult<CompletionType> {
        let key = context.vm.pop();
        let receiver = context.vm.pop();
        let value = context.vm.pop();
        let object = access_base(&value, |_| key_text(&key), false, context)?;

        let key = key.to_property_key(context)?;

        // Fast Path
        if object.is_array() {
            if let PropertyKey::Index(index) = &key {
                let object_borrowed = object.borrow();
                if let Some(element) = object_borrowed.properties().get_dense_property(index.get())
                {
                    context.vm.push(element);
                    return Ok(CompletionType::Normal);
                }
            }
        }

        // Slow path:
        let result = object.__get__(&key, receiver, &mut InternalMethodContext::new(context))?;

        context.vm.push(result);
        Ok(CompletionType::Normal)
    }
}

/// `GetPropertyByValuePush` implements the Opcode Operation for `Opcode::GetPropertyByValuePush`
///
/// Operation:
///  - Get a property by value from an object an push the key and value on the stack.
#[derive(Debug, Clone, Copy)]
pub(crate) struct GetPropertyByValuePush;

impl Operation for GetPropertyByValuePush {
    const NAME: &'static str = "GetPropertyByValuePush";
    const INSTRUCTION: &'static str = "INST - GetPropertyByValuePush";
    const COST: u8 = 4;

    fn execute(context: &mut Context) -> JsResult<CompletionType> {
        let key = context.vm.pop();
        let receiver = context.vm.pop();
        let value = context.vm.pop();
        let object = access_base(&value, |_| key_text(&key), false, context)?;

        let key = key.to_property_key(context)?;

        // Fast path:
        if object.is_array() {
            if let PropertyKey::Index(index) = &key {
                let object_borrowed = object.borrow();
                if let Some(element) = object_borrowed.properties().get_dense_property(index.get())
                {
                    context.vm.push(key);
                    context.vm.push(element);
                    return Ok(CompletionType::Normal);
                }
            }
        }

        // Slow path:
        let result = object.__get__(&key, receiver, &mut InternalMethodContext::new(context))?;

        context.vm.push(key);
        context.vm.push(result);
        Ok(CompletionType::Normal)
    }
}
