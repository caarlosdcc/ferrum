/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Protocolo MCP: mensajes JSON-RPC 2.0, uno por línea, y la lista de
//! herramientas que ofrece Ferrum.

use serde_json::{Value, json};

/// Versiones del protocolo que entiende Ferrum, de la más nueva a la más vieja.
const PROTOCOL_VERSIONS: [&str; 3] = ["2025-06-18", "2025-03-26", "2024-11-05"];

const INSTRUCTIONS: &str = "Ferrum es un navegador privado (Rust y Servo) que se abre en \
una ventana visible en cuanto usas una herramienta. Flujo habitual: navigate → read_page (texto \
y elementos con coordenadas) → click / type_text / press_key → screenshot para comprobar. Las \
coordenadas son píxeles de la página, los mismos de screenshot sin la interfaz. Nada se guarda \
al cerrar: cada sesión empieza sin cookies.";

/// Qué hacer con un mensaje recibido.
#[derive(Debug, PartialEq)]
pub enum Message {
    /// Responder ya, sin tocar el navegador.
    Reply(Value),
    /// Llamada a una herramienta.
    Call(Call),
    /// El cliente ya no quiere la respuesta de esta petición.
    Cancel(Value),
    /// Notificaciones y respuestas, que no llevan contestación.
    Ignore,
}

#[derive(Debug, PartialEq)]
pub struct Call {
    pub id: Value,
    pub name: String,
    pub arguments: Value,
}

pub fn parse(line: &str) -> Message {
    let Ok(message) = serde_json::from_str::<Value>(line) else {
        return Message::Reply(error(Value::Null, -32700, "JSON no válido"));
    };
    let method = message.get("method").and_then(Value::as_str);
    let params = message.get("params").cloned().unwrap_or(Value::Null);
    let Some(method) = method else {
        // Respuestas del cliente: Ferrum nunca le pregunta nada.
        return Message::Ignore;
    };
    if method == "notifications/cancelled" {
        return params
            .get("requestId")
            .cloned()
            .map_or(Message::Ignore, Message::Cancel);
    }
    let Some(id) = message.get("id").cloned() else {
        return Message::Ignore;
    };
    match method {
        "initialize" => Message::Reply(success(id, initialize(&params))),
        "ping" => Message::Reply(success(id, json!({}))),
        "tools/list" => Message::Reply(success(id, json!({ "tools": tools() }))),
        "tools/call" => match params.get("name").and_then(Value::as_str) {
            Some(name) => Message::Call(Call {
                id,
                name: name.to_owned(),
                arguments: params
                    .get("arguments")
                    .cloned()
                    .filter(Value::is_object)
                    .unwrap_or_else(|| json!({})),
            }),
            None => Message::Reply(error(id, -32602, "falta el nombre de la herramienta")),
        },
        other => Message::Reply(error(id, -32601, &format!("método desconocido: {other}"))),
    }
}

fn initialize(params: &Value) -> Value {
    let version = params
        .get("protocolVersion")
        .and_then(Value::as_str)
        .filter(|version| PROTOCOL_VERSIONS.contains(version))
        .unwrap_or(PROTOCOL_VERSIONS[0]);
    json!({
        "protocolVersion": version,
        "capabilities": { "tools": {} },
        "serverInfo": {
            "name": "ferrum",
            "title": "Ferrum",
            "version": env!("CARGO_PKG_VERSION"),
        },
        "instructions": INSTRUCTIONS,
    })
}

pub fn success(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

pub fn error(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

/// Resultado de una herramienta.
#[derive(Debug, PartialEq)]
pub enum Output {
    Text(String),
    Image { png_base64: String, caption: String },
    Error(String),
}

impl Output {
    pub fn into_result(self) -> Value {
        match self {
            Output::Text(text) => json!({ "content": [{ "type": "text", "text": text }] }),
            Output::Image {
                png_base64,
                caption,
            } => json!({
                "content": [
                    { "type": "image", "data": png_base64, "mimeType": "image/png" },
                    { "type": "text", "text": caption },
                ]
            }),
            Output::Error(text) => json!({
                "content": [{ "type": "text", "text": text }],
                "isError": true,
            }),
        }
    }
}

fn tool(name: &str, description: &str, properties: Value, required: &[&str]) -> Value {
    json!({
        "name": name,
        "description": description,
        "inputSchema": {
            "type": "object",
            "properties": properties,
            "required": required,
            "additionalProperties": false,
        },
    })
}

fn tools() -> Value {
    json!([
        tool(
            "navigate",
            "Abre una dirección o una búsqueda en la pestaña activa, como la barra de \
             direcciones, y espera a que cargue.",
            json!({ "url": {
                "type": "string",
                "description": "Dirección (example.com, https://…) o texto a buscar",
            }}),
            &["url"],
        ),
        tool(
            "read_page",
            "Lee la pestaña activa: título, dirección, texto y los elementos visibles con los que \
             se puede interactuar (enlaces, botones, campos), con las coordenadas de su centro \
             para click.",
            json!({ "max_chars": {
                "type": "integer",
                "minimum": 200,
                "maximum": 50000,
                "description": "Máximo de caracteres de texto (por defecto 6000)",
            }}),
            &[],
        ),
        tool(
            "screenshot",
            "Captura la pestaña activa como PNG. Sus píxeles son las coordenadas de click y \
             scroll. Con include_ui también sale la interfaz de Ferrum (pestañas y barra de \
             direcciones); entonces las coordenadas ya no coinciden con las de la página.",
            json!({ "include_ui": {
                "type": "boolean",
                "description": "Incluir la interfaz de Ferrum (por defecto no)",
            }}),
            &[],
        ),
        tool(
            "click",
            "Hace clic en la página como un usuario: en unas coordenadas (x, y), en el primer \
             elemento que cumpla un selector CSS o en el que muestre un texto. Si el clic abre \
             otra página, espera a que cargue.",
            json!({
                "x": { "type": "number", "description": "Píxeles desde la izquierda de la página" },
                "y": { "type": "number", "description": "Píxeles desde arriba de la página" },
                "selector": { "type": "string", "description": "Selector CSS, p. ej. button[type=submit]" },
                "text": { "type": "string", "description": "Texto visible del enlace o botón" },
            }),
            &[],
        ),
        tool(
            "type_text",
            "Escribe texto, tecla a tecla, en el elemento que tiene el foco (haz clic antes en \
             el campo). Con submit pulsa Intro al final.",
            json!({
                "text": { "type": "string" },
                "submit": { "type": "boolean", "description": "Pulsar Intro después (por defecto no)" },
            }),
            &["text"],
        ),
        tool(
            "press_key",
            "Pulsa una tecla en la página: Enter, Tab, Escape, Backspace, Delete, ArrowUp, \
             ArrowDown, ArrowLeft, ArrowRight, PageUp, PageDown, Home, End o un carácter. \
             Admite modificadores, p. ej. a con Control para seleccionar todo.",
            json!({
                "key": { "type": "string" },
                "modifiers": {
                    "type": "array",
                    "items": { "type": "string", "enum": ["Control", "Shift", "Alt", "Meta"] },
                },
            }),
            &["key"],
        ),
        tool(
            "scroll",
            "Desplaza la página con la rueda del ratón. dy positivo baja y negativo sube, en \
             píxeles.",
            json!({
                "dy": { "type": "number" },
                "dx": { "type": "number", "description": "Positivo hacia la derecha" },
                "x": { "type": "number", "description": "Dónde está el ratón (por defecto, el centro)" },
                "y": { "type": "number" },
            }),
            &["dy"],
        ),
        tool(
            "select_option",
            "Elige una opción de una lista desplegable (<select>) por su texto o su valor.",
            json!({
                "selector": { "type": "string", "description": "Selector CSS de la lista" },
                "option": { "type": "string", "description": "Texto o valor de la opción" },
            }),
            &["selector", "option"],
        ),
        tool(
            "evaluate",
            "Ejecuta JavaScript en la pestaña activa y devuelve el valor de la última expresión \
             (no espera a las promesas).",
            json!({ "script": { "type": "string" } }),
            &["script"],
        ),
        tool(
            "dialog",
            "Responde al aviso de la pestaña activa (alert, confirm o prompt). Mientras no se \
             responde, la página está en pausa.",
            json!({
                "accept": { "type": "boolean", "description": "Aceptar (true) o cancelar (false)" },
                "text": { "type": "string", "description": "Respuesta escrita, para prompt" },
            }),
            &["accept"],
        ),
        tool(
            "history",
            "Atrás, adelante o recargar en la pestaña activa, y espera a que cargue.",
            json!({ "action": { "type": "string", "enum": ["back", "forward", "reload"] } }),
            &["action"],
        ),
        tool(
            "tabs",
            "Lista, abre, activa o cierra pestañas. Los índices empiezan en 1.",
            json!({
                "action": { "type": "string", "enum": ["list", "new", "select", "close"] },
                "index": { "type": "integer", "minimum": 1, "description": "Para select y close" },
                "url": { "type": "string", "description": "Para new: dirección o búsqueda (por defecto, la página de inicio)" },
            }),
            &["action"],
        ),
        tool(
            "wait",
            "Espera unos milisegundos (como mucho 10000) a que la página termine algo.",
            json!({ "ms": { "type": "integer", "minimum": 0, "maximum": 10000 } }),
            &["ms"],
        ),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reply(line: &str) -> Value {
        match parse(line) {
            Message::Reply(value) => value,
            other => panic!("se esperaba una respuesta, llegó {other:?}"),
        }
    }

    #[test]
    fn initialize_echoes_known_versions() {
        let answer = reply(
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26"}}"#,
        );
        assert_eq!(answer["id"], 1);
        assert_eq!(answer["result"]["protocolVersion"], "2025-03-26");
        assert_eq!(answer["result"]["serverInfo"]["name"], "ferrum");

        let answer = reply(
            r#"{"jsonrpc":"2.0","id":"a","method":"initialize","params":{"protocolVersion":"1999-01-01"}}"#,
        );
        assert_eq!(answer["result"]["protocolVersion"], PROTOCOL_VERSIONS[0]);
    }

    #[test]
    fn lists_tools_with_schemas() {
        let answer = reply(r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#);
        let tools = answer["result"]["tools"].as_array().expect("lista");
        assert!(tools.len() >= 10);
        for tool in tools {
            assert!(tool["name"].is_string());
            assert_eq!(tool["inputSchema"]["type"], "object");
        }
    }

    #[test]
    fn parses_calls_and_notifications() {
        let call = parse(
            r#"{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"navigate","arguments":{"url":"example.com"}}}"#,
        );
        assert_eq!(
            call,
            Message::Call(Call {
                id: json!(7),
                name: "navigate".to_owned(),
                arguments: json!({ "url": "example.com" }),
            })
        );
        assert_eq!(
            parse(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#),
            Message::Ignore
        );
        assert_eq!(
            parse(
                r#"{"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":7}}"#
            ),
            Message::Cancel(json!(7))
        );
    }

    #[test]
    fn reports_bad_input() {
        assert_eq!(reply("{no es json")["error"]["code"], -32700);
        assert_eq!(
            reply(r#"{"jsonrpc":"2.0","id":3,"method":"inventado"}"#)["error"]["code"],
            -32601
        );
    }

    #[test]
    fn errors_are_flagged() {
        let result = Output::Error("mal".to_owned()).into_result();
        assert_eq!(result["isError"], true);
        assert_eq!(result["content"][0]["text"], "mal");
    }
}
