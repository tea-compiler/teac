//! Size and alignment computation for IR data types, including struct
//! field offsets, following the layout rules the backend emits code for.

use crate::asm::error::Error;
use crate::ir;
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct StructLayout {
    pub size: i64,
    pub align: i64,
    pub field_offsets: Vec<i64>,
}

#[derive(Debug, Default, Clone)]
pub struct StructLayouts(HashMap<String, StructLayout>);

impl StructLayouts {
    pub fn from_struct_types(
        structs: &indexmap::IndexMap<String, ir::StructType>,
    ) -> Result<Self, Error> {
        let mut layouts = Self::default();
        for (name, st) in structs.iter() {
            let layout = layouts.compute_single_layout(st)?;
            layouts.insert(name.clone(), layout);
        }
        Ok(layouts)
    }

    pub fn get(&self, name: &str) -> Option<&StructLayout> {
        self.0.get(name)
    }

    fn insert(&mut self, name: String, layout: StructLayout) {
        self.0.insert(name, layout);
    }

    pub fn size_align_of(&self, dtype: &ir::Dtype) -> Result<(i64, i64), Error> {
        match dtype {
            ir::Dtype::I1 => Ok((1, 1)),
            ir::Dtype::I32 => Ok((4, 4)),
            ir::Dtype::Pointer { .. } => Ok((8, 8)),
            ir::Dtype::Array { element, length } => {
                let len = length.expect("unsized array in layout computation") as i64;
                let (size, align) = self.size_align_of(element.as_ref())?;
                Ok((len * size, align))
            }
            ir::Dtype::Struct { type_name } => {
                let layout = self
                    .get(type_name)
                    .ok_or_else(|| Error::MissingStructLayout {
                        name: type_name.clone(),
                    })?;
                Ok((layout.size, layout.align))
            }
            _ => Err(Error::UnsupportedDtype {
                dtype: dtype.clone(),
            }),
        }
    }

    fn size_align_of_member(&self, dtype: &ir::Dtype) -> Result<(i64, i64), Error> {
        match dtype {
            ir::Dtype::I1 => Ok((1, 1)),
            ir::Dtype::I32 => Ok((4, 4)),
            ir::Dtype::Pointer { .. } => Ok((8, 8)),
            ir::Dtype::Array { element, length } => {
                let len = length.expect("unsized array in layout computation") as i64;
                let (s, a) = self.size_align_of(element.as_ref())?;
                Ok((len * s, a))
            }
            ir::Dtype::Struct { type_name } => self
                .get(type_name)
                .map(|l| (l.size, l.align))
                .ok_or_else(|| Error::MissingStructLayout {
                    name: type_name.clone(),
                }),
            other => Err(Error::UnsupportedDtype {
                dtype: other.clone(),
            }),
        }
    }

    fn compute_single_layout(&self, st: &ir::StructType) -> Result<StructLayout, Error> {
        let mut field_offsets = vec![0i64; st.elements.len()];
        let mut offset = 0i64;
        let mut max_align = 1i64;

        for (i, (_, member)) in st.elements.iter().enumerate() {
            let (size, align) = self.size_align_of_member(&member.dtype)?;

            offset = align_up(offset, align);
            max_align = max_align.max(align);
            field_offsets[i] = offset;
            offset += size;
        }

        Ok(StructLayout {
            size: align_up(offset, max_align),
            align: max_align,
            field_offsets,
        })
    }
}

#[inline]
pub fn align_up(x: i64, align: i64) -> i64 {
    if align <= 1 {
        x
    } else {
        ((x + align - 1) / align) * align
    }
}
