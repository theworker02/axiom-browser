use crate::{
    vm::{
        opcode::{call::call_site_error, Operation},
        CompletionType,
    },
    Context, JsObject, JsResult, JsValue,
};

/// Returns the constructor `func`, or a `TypeError` naming the callee ("x is not a constructor").
fn constructor(context: &Context, func: &JsValue) -> JsResult<JsObject> {
    match func.as_object() {
        Some(object) if object.is_constructor() => Ok(object.clone()),
        _ => Err(call_site_error(
            context,
            "is not a constructor",
            "not a constructor",
        )),
    }
}

/// `New` implements the Opcode Operation for `Opcode::New`
///
/// Operation:
///  - Call construct on a function.
#[derive(Debug, Clone, Copy)]
pub(crate) struct New;

impl New {
    fn operation(context: &mut Context, argument_count: usize) -> JsResult<CompletionType> {
        let at = context.vm.stack.len() - argument_count;
        let cons = constructor(context, &context.vm.stack[at - 1])?;

        context.vm.push(cons.clone()); // Push new.target

        cons.__construct__(argument_count).resolve(context)?;
        Ok(CompletionType::Normal)
    }
}

impl Operation for New {
    const NAME: &'static str = "New";
    const INSTRUCTION: &'static str = "INST - New";
    const COST: u8 = 3;

    fn execute(context: &mut Context) -> JsResult<CompletionType> {
        let argument_count = context.vm.read::<u8>() as usize;
        Self::operation(context, argument_count)
    }

    fn execute_with_u16_operands(context: &mut Context) -> JsResult<CompletionType> {
        let argument_count = context.vm.read::<u16>() as usize;
        Self::operation(context, argument_count)
    }

    fn execute_with_u32_operands(context: &mut Context) -> JsResult<CompletionType> {
        let argument_count = context.vm.read::<u32>() as usize;
        Self::operation(context, argument_count)
    }
}

/// `NewSpread` implements the Opcode Operation for `Opcode::NewSpread`
///
/// Operation:
///  - Call construct on a function where the arguments contain spreads.
#[derive(Debug, Clone, Copy)]
pub(crate) struct NewSpread;

impl Operation for NewSpread {
    const NAME: &'static str = "NewSpread";
    const INSTRUCTION: &'static str = "INST - NewSpread";
    const COST: u8 = 3;

    fn execute(context: &mut Context) -> JsResult<CompletionType> {
        // Get the arguments that are stored as an array object on the stack.
        let arguments_array = context.vm.pop();
        let arguments_array_object = arguments_array
            .as_object()
            .expect("arguments array in call spread function must be an object");
        let arguments = arguments_array_object
            .borrow()
            .properties()
            .to_dense_indexed_properties()
            .expect("arguments array in call spread function must be dense");

        let func = context.vm.pop();

        let cons = constructor(context, &func)?;

        let argument_count = arguments.len();
        context.vm.push(func);
        context.vm.push_values(&arguments);
        context.vm.push(cons.clone()); // Push new.target

        cons.__construct__(argument_count).resolve(context)?;
        Ok(CompletionType::Normal)
    }
}
