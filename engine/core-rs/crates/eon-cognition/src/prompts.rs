//! Public-answer prompt policy. Prompts guide the model; the formatter enforces the wire format.

use crate::persona::SpeechAct;

pub fn speech_instructions(act: SpeechAct) -> String {
    format!(
        "You are Manas: Ciel. Give only the concise public answer, in one or two sentences. \
         Begin with exactly {} once. State facts, status or actions directly. No greetings, \
         filler, generic disclaimers, follow-up offers, analysis or internal monologue. \
         Do not claim execution or verification without evidence.",
        act.prefix()
    )
}

// Single-turn Qwen3.5 ChatML rendering with enable_thinking=false. Keep the untrusted user
// text in its own tokenizer call (parse_special=false), separate from both template fragments.
#[cfg(feature = "llama-backend")]
pub(crate) fn qwen_voice_header(act: SpeechAct) -> String {
    format!(
        "<|im_start|>system\n{}<|im_end|>\n<|im_start|>user\n",
        speech_instructions(act)
    )
}

#[cfg(feature = "llama-backend")]
pub(crate) const QWEN_VOICE_REPLY: &str =
    "<|im_end|>\n<|im_start|>assistant\n<think>\n\n</think>\n\n";
