use anyhow::Context;

use super::scheduler_windows::SCHEDULER_OUTPUT_LIMIT;

pub(super) fn decode_scheduler_xml_value(value: &str) -> anyhow::Result<String> {
    let mut decoded = String::with_capacity(value.len());
    let mut remaining = value;
    while let Some(ampersand) = remaining.find('&') {
        let literal = &remaining[..ampersand];
        anyhow::ensure!(
            !literal.contains('<'),
            "scheduler Query XML contains nested markup"
        );
        decoded.push_str(literal);
        let entity = &remaining[ampersand..];
        let semicolon = entity
            .find(';')
            .context("scheduler Query XML contains an unterminated entity")?;
        decoded.push(match &entity[..=semicolon] {
            "&amp;" => '&',
            "&lt;" => '<',
            "&gt;" => '>',
            "&quot;" => '"',
            "&apos;" => '\'',
            unknown => anyhow::bail!("scheduler Query XML contains unknown entity {unknown}"),
        });
        remaining = &entity[semicolon + 1..];
    }
    anyhow::ensure!(
        !remaining.contains('<'),
        "scheduler Query XML contains nested markup"
    );
    decoded.push_str(remaining);
    Ok(decoded)
}

#[allow(clippy::too_many_lines)] // 256 lines at #779; split tracked in #778
pub(super) fn scheduler_direct_exec_values(xml: &str) -> anyhow::Result<Vec<(String, String)>> {
    anyhow::ensure!(
        xml.len() <= SCHEDULER_OUTPUT_LIMIT,
        "scheduler Query XML exceeds the bounded output limit"
    );
    let valid_name = |name: &str| {
        let mut bytes = name.bytes();
        bytes
            .next()
            .is_some_and(|byte| byte.is_ascii_alphabetic() || matches!(byte, b'_' | b':'))
            && bytes.all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b':' | b'.' | b'-')
            })
    };
    let mut cursor = 0;
    let mut stack: Vec<&str> = Vec::new();
    let mut seen_root = false;
    let mut seen_declaration = false;
    let mut actions_count = 0;
    let mut execs = Vec::new();
    let mut command = None;
    let mut arguments = None;
    let mut capture: Option<(&str, String)> = None;

    while cursor < xml.len() {
        let start = xml[cursor..]
            .find('<')
            .map(|offset| cursor + offset)
            .unwrap_or(xml.len());
        let text = &xml[cursor..start];
        if let Some((_, value)) = capture.as_mut() {
            value.push_str(text);
        } else if stack.is_empty() {
            anyhow::ensure!(
                text.trim().is_empty(),
                "scheduler Query XML has text outside the Task root"
            );
        }
        if start == xml.len() {
            cursor = start;
            break;
        }

        if xml[start..].starts_with("<!--") {
            let comment = &xml[start + "<!--".len()..];
            let end = comment
                .find("-->")
                .context("scheduler Query XML has an unterminated comment")?;
            anyhow::ensure!(
                !comment[..end].contains("--"),
                "scheduler Query XML has a malformed comment"
            );
            cursor = start + "<!--".len() + end + "-->".len();
            continue;
        }
        if xml[start..].starts_with("<?") {
            anyhow::ensure!(
                stack.is_empty()
                    && !seen_root
                    && !seen_declaration
                    && xml[start..].starts_with("<?xml"),
                "scheduler Query XML has an unsupported processing instruction"
            );
            let end = xml[start + 2..]
                .find("?>")
                .context("scheduler Query XML has an unterminated declaration")?;
            seen_declaration = true;
            cursor = start + 2 + end + 2;
            continue;
        }
        anyhow::ensure!(
            !xml[start..].starts_with("<!"),
            "scheduler Query XML has unsupported markup"
        );

        let mut quote = None;
        let mut end = None;
        for (offset, character) in xml[start + 1..].char_indices() {
            if let Some(expected) = quote {
                anyhow::ensure!(
                    character != '<',
                    "scheduler Query XML has malformed tag attributes"
                );
                if character == expected {
                    quote = None;
                }
            } else {
                match character {
                    '\'' | '"' => quote = Some(character),
                    '>' => {
                        end = Some(start + 1 + offset);
                        break;
                    }
                    '<' => anyhow::bail!("scheduler Query XML has a malformed tag"),
                    _ => {}
                }
            }
        }
        let end = end.context("scheduler Query XML has an unterminated tag")?;
        let raw = xml[start + 1..end].trim();
        cursor = end + 1;

        if let Some(closing) = raw.strip_prefix('/') {
            let name = closing.trim();
            anyhow::ensure!(
                valid_name(name) && name.len() == closing.len(),
                "scheduler Query XML has a malformed closing tag"
            );
            anyhow::ensure!(
                stack.last() == Some(&name),
                "scheduler Query XML has mismatched element nesting"
            );
            if matches!(name, "Command" | "Arguments") {
                let (kind, value) = capture
                    .take()
                    .context("scheduler Exec value did not close directly")?;
                anyhow::ensure!(kind == name, "scheduler Exec values closed out of order");
                let decoded = decode_scheduler_xml_value(&value)?;
                if name == "Command" {
                    command = Some(decoded);
                } else {
                    arguments = Some(decoded);
                }
            } else if name == "Exec" && stack.as_slice() == ["Task", "Actions", "Exec"] {
                execs.push((
                    command
                        .take()
                        .context("scheduler Exec action has no Command")?,
                    arguments
                        .take()
                        .context("scheduler Exec action has no Arguments")?,
                ));
            }
            stack.pop();
            continue;
        }

        let (open, self_closing) = raw
            .strip_suffix('/')
            .map_or((raw, false), |open| (open.trim_end(), true));
        let name_end = open.find(char::is_whitespace).unwrap_or(open.len());
        let name = &open[..name_end];
        anyhow::ensure!(
            valid_name(name),
            "scheduler Query XML has a malformed element name"
        );
        let mut attributes = &open[name_end..];
        let mut attribute_names = Vec::new();
        loop {
            attributes = attributes.trim_start();
            if attributes.is_empty() {
                break;
            }
            let name_end = attributes
                .find(|character: char| character.is_whitespace() || character == '=')
                .unwrap_or(attributes.len());
            let attribute_name = &attributes[..name_end];
            anyhow::ensure!(
                valid_name(attribute_name) && !attribute_names.contains(&attribute_name),
                "scheduler Query XML has a malformed or duplicate attribute"
            );
            attribute_names.push(attribute_name);
            attributes = attributes[name_end..].trim_start();
            attributes = attributes
                .strip_prefix('=')
                .context("scheduler Query XML attribute has no equals sign")?
                .trim_start();
            let delimiter = attributes
                .chars()
                .next()
                .filter(|character| matches!(character, '\'' | '"'))
                .context("scheduler Query XML attribute value is not quoted")?;
            attributes = &attributes[delimiter.len_utf8()..];
            let value_end = attributes
                .find(delimiter)
                .context("scheduler Query XML attribute value is unterminated")?;
            anyhow::ensure!(
                !attributes[..value_end].contains('<'),
                "scheduler Query XML attribute value contains markup"
            );
            attributes = &attributes[value_end + delimiter.len_utf8()..];
            anyhow::ensure!(
                attributes.is_empty() || attributes.chars().next().is_some_and(char::is_whitespace),
                "scheduler Query XML attributes are not separated"
            );
        }

        anyhow::ensure!(
            capture.is_none(),
            "scheduler Exec value contains nested markup"
        );
        anyhow::ensure!(
            stack.as_slice() != ["Task", "Actions"] || name == "Exec",
            "scheduler Actions contains a non-Exec direct child"
        );
        if stack.is_empty() {
            anyhow::ensure!(
                !seen_root && name == "Task" && !self_closing,
                "scheduler Query XML does not have one complete Task root"
            );
            seen_root = true;
        }
        if name == "Actions" {
            anyhow::ensure!(
                stack.as_slice() == ["Task"] && !self_closing,
                "scheduler Actions container is not a direct Task child"
            );
            actions_count += 1;
        } else if name == "Exec" {
            anyhow::ensure!(
                stack.as_slice() == ["Task", "Actions"] && !self_closing,
                "scheduler Exec action is not a direct Actions child"
            );
            command = None;
            arguments = None;
        } else if matches!(name, "Command" | "Arguments") {
            anyhow::ensure!(
                stack.as_slice() == ["Task", "Actions", "Exec"],
                "scheduler Exec value is not a direct Exec child"
            );
            let value = if name == "Command" {
                &mut command
            } else {
                &mut arguments
            };
            anyhow::ensure!(
                value.is_none(),
                "scheduler Exec action has a duplicate {name}"
            );
            if self_closing {
                *value = Some(String::new());
            } else {
                capture = Some((name, String::new()));
            }
        }
        if !self_closing {
            stack.push(name);
        }
    }

    anyhow::ensure!(
        cursor == xml.len(),
        "scheduler Query XML was not fully scanned"
    );
    anyhow::ensure!(
        seen_root && stack.is_empty() && capture.is_none(),
        "scheduler Query XML has incomplete element nesting"
    );
    anyhow::ensure!(
        actions_count == 1,
        "scheduler Query XML must have exactly one direct Actions container"
    );
    Ok(execs)
}
