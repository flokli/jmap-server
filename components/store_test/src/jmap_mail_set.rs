use std::{collections::HashMap, fs, iter::FromIterator, path::PathBuf};

use jmap_mail::{
    JMAPMailBodyProperties, JMAPMailLocalStore, JMAPMailProperties, JMAPMailStoreGetArguments,
};
use jmap_store::{
    id::{BlobId, JMAPIdSerialize},
    json::JSONValue,
    JMAPGet, JMAPId, JMAPSet, JMAP_MAILBOX,
};
use store::batch::DocumentWriter;

use crate::jmap_mail_get::UntaggedJSONValue;

impl<'x> From<UntaggedJSONValue> for JSONValue {
    fn from(value: UntaggedJSONValue) -> Self {
        match value {
            UntaggedJSONValue::Null => JSONValue::Null,
            UntaggedJSONValue::Bool(b) => JSONValue::Bool(b),
            UntaggedJSONValue::String(s) => JSONValue::String(s),
            UntaggedJSONValue::Number(n) => JSONValue::Number(n),
            UntaggedJSONValue::Array(a) => {
                JSONValue::Array(a.into_iter().map(JSONValue::from).collect())
            }
            UntaggedJSONValue::Object(o) => JSONValue::Object(
                o.into_iter()
                    .map(|(k, v)| (k, JSONValue::from(v)))
                    .collect(),
            ),
        }
    }
}

fn store_blobs<'x>(mail_store: &impl JMAPMailLocalStore<'x>, value: &mut JSONValue) {
    match value {
        JSONValue::Object(o) => {
            for (k, v) in o.iter_mut() {
                if k == "blobId" {
                    if let JSONValue::String(value) = v {
                        *value = mail_store
                            .upload_blob(0, value.as_bytes())
                            .unwrap()
                            .to_jmap_string();
                    } else {
                        panic!("blobId is not a string");
                    }
                } else {
                    store_blobs(mail_store, v);
                }
            }
        }
        JSONValue::Array(a) => {
            for v in a.iter_mut() {
                store_blobs(mail_store, v);
            }
        }
        _ => {}
    }
}

fn replace_boundaries(mut string: String) -> String {
    let mut last_pos = 0;
    let mut boundaries = Vec::new();

    while let Some(pos) = string[last_pos..].find("boundary=") {
        let mut boundary = string[last_pos + pos..].split('"').nth(1).unwrap();
        if boundary.ends_with('\\') {
            boundary = &boundary[..boundary.len() - 1];
        }
        boundaries.push(boundary.to_string());
        last_pos += pos + 9;
    }

    if !boundaries.is_empty() {
        for (pos, boundary) in boundaries.into_iter().enumerate() {
            string = string.replace(&boundary, &format!("boundary_{}", pos));
        }
    }

    string
}

fn assert_diff(str1: &str, str2: &str, filename: &str) {
    for ((pos1, ch1), (pos2, ch2)) in str1.char_indices().zip(str2.char_indices()) {
        if ch1 != ch2 {
            panic!(
                "{:?} != {:?} ({})",
                &str1[if pos1 >= 10 { pos1 - 10 } else { pos1 }..pos1 + 10],
                &str2[if pos2 >= 10 { pos2 - 10 } else { pos2 }..pos2 + 10],
                filename
            );
        }
    }

    assert_eq!(str1.len(), str2.len(), "{}", filename);
}

pub fn test_jmap_mail_set<T>(mail_store: T)
where
    T: for<'x> JMAPMailLocalStore<'x>,
{
    // TODO use mailbox create API
    mail_store
        .update_document(
            0,
            DocumentWriter::insert(
                JMAP_MAILBOX,
                mail_store
                    .assign_document_id(0, JMAP_MAILBOX, None)
                    .unwrap(),
            ),
            None,
        )
        .unwrap();
    mail_store
        .update_document(
            0,
            DocumentWriter::insert(
                JMAP_MAILBOX,
                mail_store
                    .assign_document_id(0, JMAP_MAILBOX, None)
                    .unwrap(),
            ),
            None,
        )
        .unwrap();

    test_jmap_mail_create(&mail_store);
    test_jmap_mail_update(&mail_store);
}

fn test_jmap_mail_create<'x>(mail_store: &'x impl JMAPMailLocalStore<'x>) {
    let mut test_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    test_dir.push("resources");
    test_dir.push("jmap_mail_set");

    for file_name in fs::read_dir(&test_dir).unwrap() {
        let mut file_name = file_name.as_ref().unwrap().path();
        if file_name.extension().map_or(true, |e| e != "json") {
            continue;
        }

        let result = mail_store
            .mail_set(JMAPSet {
                account_id: 0,
                if_in_state: None,
                create: Some(HashMap::from_iter(
                    vec![(
                        "1".to_string(),
                        JSONValue::from(
                            serde_json::from_slice::<UntaggedJSONValue>(
                                &fs::read(&file_name).unwrap(),
                            )
                            .unwrap(),
                        )
                        .unwrap_object()
                        .into_iter()
                        .map(|(k, mut v)| {
                            store_blobs(mail_store, &mut v);
                            (JMAPMailProperties::parse(&k).unwrap(), v)
                        })
                        .collect::<HashMap<JMAPMailProperties, JSONValue>>(),
                    )]
                    .into_iter(),
                )),
                update: None,
                destroy: None,
            })
            .unwrap();

        assert!(
            result.not_created.is_none(),
            "{:?}",
            result.not_created.unwrap()
        );

        let values = result
            .created
            .as_ref()
            .unwrap()
            .get("1")
            .unwrap()
            .to_object()
            .unwrap();

        let raw_message = mail_store
            .download_blob(
                0,
                BlobId::from_jmap_string(values.get("blobId").unwrap().to_string().unwrap())
                    .unwrap(),
            )
            .unwrap()
            .unwrap();

        let parsed_message = UntaggedJSONValue::from(
            mail_store
                .mail_get(
                    JMAPGet {
                        account_id: 0,
                        ids: vec![JMAPId::from_jmap_string(
                            values.get("id").unwrap().to_string().unwrap(),
                        )
                        .unwrap()]
                        .into(),
                        properties: vec![
                            JMAPMailProperties::Id,
                            JMAPMailProperties::BlobId,
                            JMAPMailProperties::ThreadId,
                            JMAPMailProperties::MailboxIds,
                            JMAPMailProperties::Keywords,
                            JMAPMailProperties::ReceivedAt,
                            JMAPMailProperties::MessageId,
                            JMAPMailProperties::InReplyTo,
                            JMAPMailProperties::References,
                            JMAPMailProperties::Sender,
                            JMAPMailProperties::From,
                            JMAPMailProperties::To,
                            JMAPMailProperties::Cc,
                            JMAPMailProperties::Bcc,
                            JMAPMailProperties::ReplyTo,
                            JMAPMailProperties::Subject,
                            JMAPMailProperties::SentAt,
                            JMAPMailProperties::HasAttachment,
                            JMAPMailProperties::Preview,
                            JMAPMailProperties::BodyValues,
                            JMAPMailProperties::TextBody,
                            JMAPMailProperties::HtmlBody,
                            JMAPMailProperties::Attachments,
                            JMAPMailProperties::BodyStructure,
                        ]
                        .into(),
                    },
                    JMAPMailStoreGetArguments {
                        body_properties: vec![
                            JMAPMailBodyProperties::PartId,
                            JMAPMailBodyProperties::BlobId,
                            JMAPMailBodyProperties::Size,
                            JMAPMailBodyProperties::Name,
                            JMAPMailBodyProperties::Type,
                            JMAPMailBodyProperties::Charset,
                            JMAPMailBodyProperties::Headers,
                            JMAPMailBodyProperties::Disposition,
                            JMAPMailBodyProperties::Cid,
                            JMAPMailBodyProperties::Language,
                            JMAPMailBodyProperties::Location,
                        ],
                        fetch_text_body_values: true,
                        fetch_html_body_values: true,
                        fetch_all_body_values: true,
                        max_body_value_bytes: 100,
                    },
                )
                .unwrap()
                .list,
        );

        file_name.set_extension("jmap");

        assert_diff(
            &replace_boundaries(serde_json::to_string_pretty(&parsed_message).unwrap()),
            &String::from_utf8(fs::read(&file_name).unwrap()).unwrap(),
            file_name.to_str().unwrap(),
        );

        /*fs::write(
            file_name.clone(),
            replace_boundaries(serde_json::to_string_pretty(&parsed_message).unwrap()),
        )
        .unwrap();*/

        file_name.set_extension("eml");

        assert_diff(
            &replace_boundaries(String::from_utf8(raw_message).unwrap()),
            &String::from_utf8(fs::read(&file_name).unwrap()).unwrap(),
            file_name.to_str().unwrap(),
        );

        /*fs::write(
            file_name,
            replace_boundaries(String::from_utf8(raw_message).unwrap()),
        )
        .unwrap();*/
    }
}

fn test_jmap_mail_update<'x>(mail_store: &'x impl JMAPMailLocalStore<'x>) {}
