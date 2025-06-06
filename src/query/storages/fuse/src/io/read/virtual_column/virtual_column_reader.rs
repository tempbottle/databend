// Copyright 2021 Datafuse Labs
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::collections::HashSet;
use std::sync::Arc;

use databend_common_catalog::plan::DataSourcePlan;
use databend_common_catalog::plan::PushDownInfo;
use databend_common_catalog::plan::VirtualColumnInfo;
use databend_common_catalog::table_context::TableContext;
use databend_common_exception::Result;
use databend_common_expression::ColumnId;
use databend_common_expression::TableSchemaRef;
use databend_storages_common_table_meta::table::TableCompression;
use opendal::Operator;

// Virtual columns are generated by extracting internal fields of Variant values.
// For example the following data:
// {
//     "id": 1,
//     "name": "databend",
//     "tags": ["powerful", "fast"],
//     "pricings": [
//         {
//             "type": "Standard",
//             "price": "Pay as you go"
//         },
//         {
//             "type": "Enterprise",
//             "price": "Custom"
//         }
//     ]
// }
// We can extract these fields `val['id']`, `val['name']`, `val['tags'][0]`,
// `val['pricings'][0]['type']` and so on as virtual columns,
// and and store them a separate block file.
//
// When reading virtual columns, first check whether the block file of virtual columns exists,
// if it exists, read the schema from the meta of the file and using the schema
// to determine whether the virtual columns to be read are generated or not,
// if they have been generated, we can read them directly,
// otherwise, extract them from the source columns.
//
// If all the virtual columns of the source column have been generated,
// and this source colume does not need to be read, the source column
// can be ignored when reading.
#[derive(Clone)]
pub struct VirtualColumnReader {
    pub(super) ctx: Arc<dyn TableContext>,
    pub(super) dal: Operator,
    pub(super) source_schema: TableSchemaRef,
    pub prewhere_schema: Option<TableSchemaRef>,
    pub output_schema: TableSchemaRef,
    pub(super) compression: TableCompression,
    pub virtual_column_info: VirtualColumnInfo,
}

impl VirtualColumnReader {
    pub fn try_create(
        ctx: Arc<dyn TableContext>,
        dal: Operator,
        source_schema: TableSchemaRef,
        plan: &DataSourcePlan,
        virtual_column_info: VirtualColumnInfo,
        compression: TableCompression,
    ) -> Result<Self> {
        let prewhere_schema =
            if let Some(v) = PushDownInfo::prewhere_of_push_downs(plan.push_downs.as_ref()) {
                let prewhere_schema = v
                    .prewhere_columns
                    .project_schema(plan.source_info.schema().as_ref());
                Some(Arc::new(prewhere_schema))
            } else {
                None
            };
        let output_schema = plan.schema();

        Ok(Self {
            ctx,
            dal,
            source_schema,
            prewhere_schema,
            output_schema,
            compression,
            virtual_column_info,
        })
    }

    pub(super) fn generate_ignore_column_ids(
        &self,
        ignored_source_column_ids: &HashSet<ColumnId>,
    ) -> Option<HashSet<ColumnId>> {
        if !ignored_source_column_ids.is_empty() {
            let mut ignore_column_ids = HashSet::new();
            for column_id in ignored_source_column_ids {
                // If the source column is not needed, we can ignore it to reduce IO.
                if self.output_schema.is_column_deleted(*column_id) {
                    if let Some(prewhere_schema) = &self.prewhere_schema {
                        if !prewhere_schema.is_column_deleted(*column_id) {
                            continue;
                        }
                    }
                    ignore_column_ids.insert(*column_id);
                }
            }
            if !ignore_column_ids.is_empty() {
                Some(ignore_column_ids)
            } else {
                None
            }
        } else {
            None
        }
    }
}
