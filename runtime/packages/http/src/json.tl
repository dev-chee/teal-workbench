@edition("2026");

host "std.codec" as codec {
    fn quote(text: string) -> string | Error;

    fn validate(text: string) -> string | Error;

    fn get(text: string, pointer: string) -> string | Error;
}
/// 将字符串编码为 JSON 字符串值，处理所有控制字符和引号。
pub fn quote(text: string) -> string | Error {
    return codec::quote(text) ?;
}

/// 校验并规范化 JSON；超过大小或嵌套上限时失败。
pub fn validate(text: string) -> string | Error {
    return codec::validate(text) ?;
}

/// 读取任意 JSON 值的规范文本；不存在的 Pointer 抛错。
pub fn get(text: string, pointer: string) -> string | Error {
    return codec::get(text, pointer) ?;
}

/// 从已编码的值构造数组；每个值都经过校验。
pub fn array(values: string[]) -> string | Error {
    let encoded: string[] = [];
    for value in values {
        encoded.push(validate(value) ?);
    }
    return "[" + encoded.join(",") + "]";
}
/// 一个对象字段；value 是编码后的 JSON，不是裸字符串。
pub class Field { pub name: string, pub value: string }
/// 构造对象，拒绝重复字段，防止请求含义依赖服务端解析器。
pub fn object(fields: Field[]) -> string | Error {
    let names: string[] = [];
    let encoded: string[] = [];
    for field in fields {
        if names.contains(field.name) {
            return Error::make("json", "Duplicate JSON field", ErrorOrigin.user, false);
        }
        names.push(field.name);
        encoded.push(quote(field.name) ? +":" + validate(field.value) ?);
    }
    return "{{" + encoded.join(",") + "}}";
}

/// 构造字符串字段。
pub fn text(name: string, value: string) -> Field | Error {
    return Field { name: name, value: quote(value) ? };
}

/// 构造任意 JSON 字段。
pub fn value(name: string, encoded: string) -> Field | Error {
    return Field { name: name, value: validate(encoded) ? };
}
