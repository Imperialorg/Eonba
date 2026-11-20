"""K2 diagnostic-guided token edits. This is not a Kotlin tree-sitter parser."""

import re


IDENT = r"[A-Za-z_]\w*"
PRIMITIVES = {"Byte", "Short", "Int", "Long", "Float", "Double", "Char", "Boolean"}


def propose(src, diagnostic):
    code = diagnostic["code"]
    pos = diagnostic["offset"]
    msg = diagnostic["message"]
    before, after = src[:pos], src[pos:]
    line = before.rsplit("\n", 1)[-1]
    word = re.match(IDENT, after)
    if code == "UNSAFE_CALL" and after.startswith(".") and re.search(r":\s*[\w<>]+\?\s*=\s*\w+$", line):
        return before + "?" + after
    if code == "ARGUMENT_TYPE_MISMATCH" and word:
        types = re.search(r"actual type is '(\w+)', but '(\w+)' was expected", msg)
        callee = re.search(rf"({IDENT})\($", line)
        if not (types and callee and after[word.end():].startswith(")")):
            return None
        actual, expected = types.groups()
        if actual == expected + "Array" and expected in PRIMITIVES and re.search(
            rf"\bfun\s+{callee[1]}\(vararg\s+{IDENT}\s*:\s*{expected}\)", src
        ):
            return before + "*" + after
        # Widen only; do not silently truncate or change floating-point precision.
        if actual in {"Byte", "Short", "Int"} and expected == "Long":
            return before + word[0] + ".toLong()" + after[word.end():]
    if code == "CONST_VAL_WITH_NON_CONST_INITIALIZER" and re.fullmatch(rf"const\s+val\s+{IDENT}\s*=\s*", line):
        return before[:-len(line)] + line.removeprefix("const").lstrip() + after
    if code == "SUPERTYPE_NOT_INITIALIZED" and word and re.match(r"\s*\{", after[word.end():]):
        if re.search(rf"\bopen class {word[0]}\s*\{{", src):
            return before + word[0] + "()" + after[word.end():]
    if code == "OPERATOR_MODIFIER_REQUIRED":
        decl = re.search(r"'fun (\w+)\(", msg)
        if decl and decl[1] in {"get", "set"}:
            matches = list(re.finditer(rf"\bfun\s+{decl[1]}\(", src))
            if len(matches) == 1:
                i = matches[0].start()
                return src[:i] + "operator " + src[i:]
    if code == "DATA_CLASS_NOT_PROPERTY_PARAMETER" and re.match(rf"{IDENT}\s*:", after):
        if re.search(r"\bdata class \w+\([^)]*$", line):
            return before + "val " + after
    if code == "DELEGATE_SPECIAL_FUNCTION_MISSING" and "'setValue(" in msg and re.match(r"by\s+lazy\b", after):
        binding = re.fullmatch(rf"(\s*)var(\s+({IDENT})\s+)", line)
        if binding and not re.search(rf"\b{binding[3]}\s*=(?!=)", src):
            return before[:-len(line)] + binding[1] + "val" + binding[2] + after
    if code == "RETURN_TYPE_MISMATCH":
        mismatch = re.search(r"expected '(\w+)Array', actual 'Array<(\w+)>'", msg)
        expr = after.splitlines()[0]
        if mismatch and mismatch[1] == mismatch[2] and mismatch[1] in PRIMITIVES and expr.endswith(".toTypedArray()"):
            return before + expr.removesuffix(".toTypedArray()") + f".to{mismatch[1]}Array()" + after[len(expr):]
    if code == "ILLEGAL_ESCAPE" and re.search(r'\bRegex\("[^"\\]*$', line) and re.match(r"\\[dDsSwW]", after):
        return before + "\\" + after
    return None
