/*
 * Copyright (c) 2020-2022, Stalwart Labs Ltd.
 *
 * This file is part of the Stalwart JMAP Server.
 *
 * This program is free software: you can redistribute it and/or modify
 * it under the terms of the GNU Affero General Public License as
 * published by the Free Software Foundation, either version 3 of
 * the License, or (at your option) any later version.
 *
 * This program is distributed in the hope that it will be useful,
 * but WITHOUT ANY WARRANTY; without even the implied warranty of
 * MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the
 * GNU Affero General Public License for more details.
 * in the LICENSE file at the top-level directory of this distribution.
 * You should have received a copy of the GNU Affero General Public License
 * along with this program.  If not, see <http://www.gnu.org/licenses/>.
 *
 * You can be released from the requirements of the AGPLv3 license by
 * purchasing a commercial license. Please contact licensing@stalw.art
 * for more details.
*/

use std::{
    sync::{Arc, Mutex},
    time::Instant,
};

use store::ahash::AHashMap;
use store::{
    core::{collection::Collection, document::Document, JMAPIdPrefix},
    nlp::Language,
    read::{
        comparator::Comparator,
        filter::{ComparisonOperator, Filter, Query},
        FilterMapper,
    },
    write::{
        batch::WriteBatch,
        options::{IndexOptions, Options},
    },
};
use store::{JMAPStore, Store};

use crate::tests::store::utils::deflate_artwork_data;

pub const FIELDS: [&str; 20] = [
    "id",
    "accession_number",
    "artist",
    "artistRole",
    "artistId",
    "title",
    "dateText",
    "medium",
    "creditLine",
    "year",
    "acquisitionYear",
    "dimensions",
    "width",
    "height",
    "depth",
    "units",
    "inscription",
    "thumbnailCopyright",
    "thumbnailUrl",
    "url",
];

enum FieldType {
    Keyword,
    Text,
    FullText,
    Integer,
}

const FIELDS_OPTIONS: [FieldType; 20] = [
    FieldType::Integer,  // "id",
    FieldType::Keyword,  // "accession_number",
    FieldType::Text,     // "artist",
    FieldType::Keyword,  // "artistRole",
    FieldType::Integer,  // "artistId",
    FieldType::FullText, // "title",
    FieldType::FullText, // "dateText",
    FieldType::FullText, // "medium",
    FieldType::FullText, // "creditLine",
    FieldType::Integer,  // "year",
    FieldType::Integer,  // "acquisitionYear",
    FieldType::FullText, // "dimensions",
    FieldType::Integer,  // "width",
    FieldType::Integer,  // "height",
    FieldType::Integer,  // "depth",
    FieldType::Text,     // "units",
    FieldType::FullText, // "inscription",
    FieldType::Text,     // "thumbnailCopyright",
    FieldType::Text,     // "thumbnailUrl",
    FieldType::Text,     // "url",
];

#[allow(clippy::mutex_atomic)]
pub fn test<T>(db: Arc<JMAPStore<T>>, do_insert: bool)
where
    T: for<'x> Store<'x> + 'static,
{
    rayon::ThreadPoolBuilder::new()
        .num_threads(8)
        .build()
        .unwrap()
        .scope_fifo(|s| {
            if do_insert {
                let db = Arc::new(&db);
                let now = Instant::now();
                let documents = Arc::new(Mutex::new(Vec::new()));

                for record in csv::ReaderBuilder::new()
                    .has_headers(true)
                    .from_reader(&deflate_artwork_data()[..])
                    .records()
                {
                    let record = record.unwrap();
                    let documents = documents.clone();
                    let record_id = db.assign_document_id(0, Collection::Mail).unwrap();

                    s.spawn_fifo(move |_| {
                        let mut builder = Document::new(Collection::Mail, record_id);
                        for (pos, field) in record.iter().enumerate() {
                            match FIELDS_OPTIONS[pos] {
                                FieldType::Text => {
                                    if !field.is_empty() {
                                        builder.text(
                                            pos as u8,
                                            field.to_lowercase(),
                                            Language::English,
                                            IndexOptions::new().index().tokenize(),
                                        );
                                    }
                                }
                                FieldType::FullText => {
                                    if !field.is_empty() {
                                        builder.text(
                                            pos as u8,
                                            field.to_lowercase(),
                                            Language::English,
                                            IndexOptions::new().index().full_text(0),
                                        );
                                    }
                                }
                                FieldType::Integer => {
                                    builder.number(
                                        pos as u8,
                                        field.parse::<u32>().unwrap_or(0),
                                        IndexOptions::new().store().index(),
                                    );
                                }
                                FieldType::Keyword => {
                                    if !field.is_empty() {
                                        builder.text(
                                            pos as u8,
                                            field.to_lowercase(),
                                            Language::Unknown,
                                            IndexOptions::new().store().index().keyword(),
                                        );
                                    }
                                }
                            }
                        }
                        documents.lock().unwrap().push(builder);
                    });
                }

                let mut documents = documents.lock().unwrap();
                let documents_len = documents.len();
                let mut document_chunk = Vec::new();

                println!(
                    "Parsed {} entries in {} ms.",
                    documents_len,
                    now.elapsed().as_millis()
                );

                for (pos, document) in documents.drain(..).enumerate() {
                    document_chunk.push(document);
                    if document_chunk.len() == 1000 || pos == documents_len - 1 {
                        let db = db.clone();
                        let chunk = document_chunk;
                        document_chunk = Vec::new();

                        s.spawn_fifo(move |_| {
                            let now = Instant::now();
                            let num_docs = chunk.len();
                            let mut batch = WriteBatch::new(0);
                            for document in chunk {
                                batch.insert_document(document);
                            }
                            db.write(batch).unwrap();
                            println!(
                                "Inserted {} entries in {} ms (Thread {}/{}).",
                                num_docs,
                                now.elapsed().as_millis(),
                                rayon::current_thread_index().unwrap(),
                                rayon::current_num_threads()
                            );
                        });
                    }
                }
            }
        });

    println!("Running filter tests...");
    test_filter(db.clone());

    println!("Running sort tests...");
    test_sort(db);
}

pub fn test_filter<T>(db: Arc<JMAPStore<T>>)
where
    T: for<'x> Store<'x> + 'static,
{
    let mut fields = AHashMap::default();
    for (field_num, field) in FIELDS.iter().enumerate() {
        fields.insert(field.to_string(), field_num as u8);
    }

    let tests = [
        (
            Filter::and(vec![
                Filter::new_condition(
                    fields["title"],
                    ComparisonOperator::Equal,
                    Query::match_english("water".into()),
                ),
                Filter::new_condition(
                    fields["year"],
                    ComparisonOperator::Equal,
                    Query::Integer(1979),
                ),
            ]),
            vec!["p11293"],
        ),
        (
            Filter::and(vec![
                Filter::new_condition(
                    fields["medium"],
                    ComparisonOperator::Equal,
                    Query::match_english("gelatin".into()),
                ),
                Filter::new_condition(
                    fields["year"],
                    ComparisonOperator::GreaterThan,
                    Query::Integer(2000),
                ),
                Filter::new_condition(
                    fields["width"],
                    ComparisonOperator::LowerThan,
                    Query::Integer(180),
                ),
                Filter::new_condition(
                    fields["width"],
                    ComparisonOperator::GreaterThan,
                    Query::Integer(0),
                ),
            ]),
            vec!["p79426", "p79427", "p79428", "p79429", "p79430"],
        ),
        (
            Filter::and(vec![Filter::new_condition(
                fields["title"],
                ComparisonOperator::Equal,
                Query::match_english("'rustic bridge'".into()),
            )]),
            vec!["d05503"],
        ),
        (
            Filter::and(vec![
                Filter::new_condition(
                    fields["title"],
                    ComparisonOperator::Equal,
                    Query::match_english("'rustic'".into()),
                ),
                Filter::new_condition(
                    fields["title"],
                    ComparisonOperator::Equal,
                    Query::match_english("study".into()),
                ),
            ]),
            vec!["d00399", "d05352"],
        ),
        (
            Filter::and(vec![
                Filter::new_condition(
                    fields["artist"],
                    ComparisonOperator::Equal,
                    Query::Tokenize("mauro kunst".into()),
                ),
                Filter::new_condition(
                    fields["artistRole"],
                    ComparisonOperator::Equal,
                    Query::Keyword("artist".into()),
                ),
                Filter::or(vec![
                    Filter::new_condition(
                        fields["year"],
                        ComparisonOperator::Equal,
                        Query::Integer(1969),
                    ),
                    Filter::new_condition(
                        fields["year"],
                        ComparisonOperator::Equal,
                        Query::Integer(1971),
                    ),
                ]),
            ]),
            vec!["p01764", "t05843"],
        ),
        (
            Filter::and(vec![
                Filter::not(vec![Filter::new_condition(
                    fields["medium"],
                    ComparisonOperator::Equal,
                    Query::match_english("oil".into()),
                )]),
                Filter::new_condition(
                    fields["creditLine"],
                    ComparisonOperator::Equal,
                    Query::match_english("bequeath".into()),
                ),
                Filter::or(vec![
                    Filter::and(vec![
                        Filter::new_condition(
                            fields["year"],
                            ComparisonOperator::GreaterEqualThan,
                            Query::Integer(1900),
                        ),
                        Filter::new_condition(
                            fields["year"],
                            ComparisonOperator::LowerThan,
                            Query::Integer(1910),
                        ),
                    ]),
                    Filter::and(vec![
                        Filter::new_condition(
                            fields["year"],
                            ComparisonOperator::GreaterEqualThan,
                            Query::Integer(2000),
                        ),
                        Filter::new_condition(
                            fields["year"],
                            ComparisonOperator::LowerThan,
                            Query::Integer(2010),
                        ),
                    ]),
                ]),
            ]),
            vec![
                "n02478", "n02479", "n03568", "n03658", "n04327", "n04328", "n04721", "n04739",
                "n05095", "n05096", "n05145", "n05157", "n05158", "n05159", "n05298", "n05303",
                "n06070", "t01181", "t03571", "t05805", "t05806", "t12147", "t12154", "t12155",
            ],
        ),
        (
            Filter::and(vec![
                Filter::new_condition(
                    fields["artist"],
                    ComparisonOperator::Equal,
                    Query::Tokenize("warhol".into()),
                ),
                Filter::not(vec![Filter::new_condition(
                    fields["title"],
                    ComparisonOperator::Equal,
                    Query::match_english("'campbell'".into()),
                )]),
                Filter::not(vec![Filter::or(vec![
                    Filter::new_condition(
                        fields["year"],
                        ComparisonOperator::GreaterThan,
                        Query::Integer(1980),
                    ),
                    Filter::and(vec![
                        Filter::new_condition(
                            fields["width"],
                            ComparisonOperator::GreaterThan,
                            Query::Integer(500),
                        ),
                        Filter::new_condition(
                            fields["height"],
                            ComparisonOperator::GreaterThan,
                            Query::Integer(500),
                        ),
                    ]),
                ])]),
                Filter::new_condition(
                    fields["acquisitionYear"],
                    ComparisonOperator::Equal,
                    Query::Integer(2008),
                ),
            ]),
            vec!["ar00039", "t12600"],
        ),
        (
            Filter::and(vec![
                Filter::new_condition(
                    fields["title"],
                    ComparisonOperator::Equal,
                    Query::match_english("study".into()),
                ),
                Filter::new_condition(
                    fields["medium"],
                    ComparisonOperator::Equal,
                    Query::match_english("paper".into()),
                ),
                Filter::new_condition(
                    fields["creditLine"],
                    ComparisonOperator::Equal,
                    Query::match_english("'purchased'".into()),
                ),
                Filter::not(vec![
                    Filter::new_condition(
                        fields["title"],
                        ComparisonOperator::Equal,
                        Query::match_english("'anatomical'".into()),
                    ),
                    Filter::new_condition(
                        fields["title"],
                        ComparisonOperator::Equal,
                        Query::match_english("'for'".into()),
                    ),
                ]),
                Filter::new_condition(
                    fields["year"],
                    ComparisonOperator::GreaterThan,
                    Query::Integer(1900),
                ),
                Filter::new_condition(
                    fields["acquisitionYear"],
                    ComparisonOperator::GreaterThan,
                    Query::Integer(2000),
                ),
            ]),
            vec![
                "p80042", "p80043", "p80044", "p80045", "p80203", "t11937", "t12172",
            ],
        ),
    ];

    for (filter, expected_results) in tests {
        let mut results: Vec<String> = Vec::with_capacity(expected_results.len());

        for jmap_id in db
            .query_store::<FilterMapper>(
                0,
                Collection::Mail,
                filter,
                Comparator::ascending(fields["accession_number"]),
            )
            .unwrap()
        {
            results.push(
                db.get_document_value(
                    0,
                    Collection::Mail,
                    jmap_id.get_document_id(),
                    fields["accession_number"],
                )
                .unwrap()
                .unwrap(),
            );
        }
        assert_eq!(results, expected_results);
    }
}

pub fn test_sort<T>(db: Arc<JMAPStore<T>>)
where
    T: for<'x> Store<'x> + 'static,
{
    let mut fields = AHashMap::default();
    for (field_num, field) in FIELDS.iter().enumerate() {
        fields.insert(field.to_string(), field_num as u8);
    }

    let tests = [
        (
            Filter::and(vec![
                Filter::gt(fields["year"], Query::Integer(0)),
                Filter::gt(fields["acquisitionYear"], Query::Integer(0)),
                Filter::gt(fields["width"], Query::Integer(0)),
            ]),
            vec![
                Comparator::descending(fields["year"]),
                Comparator::ascending(fields["acquisitionYear"]),
                Comparator::ascending(fields["width"]),
                Comparator::descending(fields["accession_number"]),
            ],
            vec![
                "t13655", "t13811", "p13352", "p13351", "p13350", "p13349", "p13348", "p13347",
                "p13346", "p13345", "p13344", "p13342", "p13341", "p13340", "p13339", "p13338",
                "p13337", "p13336", "p13335", "p13334", "p13333", "p13332", "p13331", "p13330",
                "p13329", "p13328", "p13327", "p13326", "p13325", "p13324", "p13323", "t13786",
                "p13322", "p13321", "p13320", "p13319", "p13318", "p13317", "p13316", "p13315",
                "p13314", "t13588", "t13587", "t13586", "t13585", "t13584", "t13540", "t13444",
                "ar01154", "ar01153",
            ],
        ),
        (
            Filter::and(vec![
                Filter::gt(fields["width"], Query::Integer(0)),
                Filter::gt(fields["height"], Query::Integer(0)),
            ]),
            vec![
                Comparator::descending(fields["width"]),
                Comparator::ascending(fields["height"]),
            ],
            vec![
                "t03681", "t12601", "ar00166", "t12625", "t12915", "p04182", "t06483", "ar00703",
                "t07671", "ar00021", "t05557", "t07918", "p06298", "p05465", "p06640", "t12855",
                "t01355", "t12800", "t12557", "t02078",
            ],
        ),
        (
            Filter::None,
            vec![
                Comparator::descending(fields["medium"]),
                Comparator::descending(fields["artistRole"]),
                Comparator::ascending(fields["accession_number"]),
            ],
            vec![
                "ar00627", "ar00052", "t00352", "t07275", "t12318", "t04931", "t13683", "t13686",
                "t13687", "t13688", "t13689", "t13690", "t13691", "t07766", "t07918", "t12993",
                "ar00044", "t13326", "t07614", "t12414",
            ],
        ),
    ];

    for (filter, sort, expected_results) in tests {
        let mut results: Vec<String> = Vec::with_capacity(expected_results.len());

        for jmap_id in db
            .query_store::<FilterMapper>(0, Collection::Mail, filter, Comparator::List(sort))
            .unwrap()
        {
            results.push(
                db.get_document_value(
                    0,
                    Collection::Mail,
                    jmap_id.get_document_id(),
                    fields["accession_number"],
                )
                .unwrap()
                .unwrap(),
            );

            if results.len() == expected_results.len() {
                break;
            }
        }
        assert_eq!(results, expected_results);
    }
}
