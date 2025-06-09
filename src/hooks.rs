use std::rc::Rc;
use yew::prelude::*;
use web_sys::HtmlInputElement;

/// Holds the state and callbacks for a validated input field.
#[derive(Clone)]
pub struct ValidatedInput<T: Clone + PartialEq + 'static> {
    /// The current text content of the input field.
    pub text: String,
    /// The current parsed and validated numeric/typed value.
    pub value: T,
    /// An optional error message if validation failed.
    pub error: Option<String>,
    /// Callback for the text input's `oninput` event. Updates the internal text state.
    pub on_text_input: Callback<InputEvent>,
    /// Callback to trigger parsing and validation of the current text.
    /// Typically used with `onchange` or after an Enter key press on the text input.
    pub on_commit: Callback<()>,
    /// Callback to programmatically set the numeric/typed value.
    /// This will also update the text representation and clear any errors.
    pub set_value: Callback<T>,
}

/// Custom hook to manage state for a validated input field.
pub fn use_validated_input<T: Clone + PartialEq + std::fmt::Display + 'static>(
    initial_value: T,
    parse_and_validate: Rc<dyn Fn(&str) -> Result<T, String>>,
) -> ValidatedInput<T> {
    // These are the actual UseStateHandles.
    let numeric_state_handle: UseStateHandle<T> = use_state(|| initial_value.clone());
    let text_state_handle: UseStateHandle<String> = use_state(|| initial_value.to_string());
    let error_state_handle: UseStateHandle<Option<String>> = use_state(|| None::<String>);

    let on_text_input = {
        // Clone the handle for the closure.
        let text_setter = text_state_handle.clone();
        Callback::from(move |e: InputEvent| {
            let input: HtmlInputElement = e.target_unchecked_into();
            text_setter.set(input.value());
        })
    };

    let on_commit = {
        // Clone handles for the closure.
        let current_text_handle = text_state_handle.clone();
        let numeric_setter = numeric_state_handle.clone();
        let text_setter_on_commit = text_state_handle.clone();
        let error_setter = error_state_handle.clone();
        let parse_fn = parse_and_validate.clone();

        Callback::from(move |_| {
            // Dereference the handle to get the current String value.
            match parse_fn(&(*current_text_handle)) {
                Ok(parsed_val) => {
                    numeric_setter.set(parsed_val.clone());
                    text_setter_on_commit.set(parsed_val.to_string()); // Update text to canonical form
                    error_setter.set(None);
                }
                Err(err_msg) => {
                    error_setter.set(Some(err_msg));
                }
            }
        })
    };

    let set_value = {
        // Clone handles for the closure.
        let numeric_setter = numeric_state_handle.clone();
        let text_setter = text_state_handle.clone();
        let error_setter = error_state_handle.clone();
        Callback::from(move |new_val: T| {
            numeric_setter.set(new_val.clone());
            text_setter.set(new_val.to_string());
            error_setter.set(None); // Assume programmatic set is valid
        })
    };

    // Effect to update text_state if numeric_state_handle's value changes programmatically
    {
        // Clone the value for the dependency array.
        let numeric_value_snapshot = (*numeric_state_handle).clone();
        // Clone handles for the effect closure.
        let text_setter_for_effect = text_state_handle.clone();
        let current_text_handle_for_effect = text_state_handle.clone();


        use_effect_with(numeric_value_snapshot, move |current_numeric_value_dep| {
            let formatted_numeric_text = current_numeric_value_dep.to_string();
            // Dereference handle to get current text for comparison.
            if *current_text_handle_for_effect != formatted_numeric_text {
                 text_setter_for_effect.set(formatted_numeric_text);
            }
            || (()) // Return a no-op destructor
        });
    }

    ValidatedInput {
        // Dereference handles to get current values for the returned struct.
        text: (*text_state_handle).clone(),
        value: (*numeric_state_handle).clone(),
        error: (*error_state_handle).clone(),
        on_text_input,
        on_commit,
        set_value,
    }
}
