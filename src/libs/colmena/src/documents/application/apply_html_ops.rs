//! HtmlOpApplier — mutates HtmlIR for each PatchOp (slide-level + block-level).
//!
//! Pure to ports: depends only on IdGenerator from domain.

use crate::documents::domain::artifact::{AssignedIds, OpOutcome};
use crate::documents::domain::ir::html::{Block, HtmlIR, ImageSrc, Slide, SlideLayout, VideoSrc};
use crate::documents::domain::patch::PatchOp;
use crate::documents::domain::{DocumentError, IdGenerator};

pub struct HtmlOpApplier<'a> {
    pub ids: &'a dyn IdGenerator,
}

impl<'a> HtmlOpApplier<'a> {
    pub fn apply(&self, ir: &mut HtmlIR, op: &PatchOp) -> Result<OpOutcome, DocumentError> {
        match op {
            PatchOp::AddSlide {
                layout,
                at_index,
                title,
                subtitle,
            } => self.add_slide(ir, *layout, *at_index, title.clone(), subtitle.clone()),
            PatchOp::DeleteSlide { slide_id } => self.delete_slide(ir, slide_id),
            PatchOp::ReorderSlides { order } => self.reorder_slides(ir, order),
            PatchOp::SetSlideLayout { slide_id, layout } => {
                self.set_slide_layout(ir, slide_id, *layout)
            }
            PatchOp::SetSlideTitle {
                slide_id,
                title,
                subtitle,
            } => self.set_slide_title(ir, slide_id, title.clone(), subtitle.clone()),
            PatchOp::SetSlideNotes { slide_id, notes } => {
                self.set_slide_notes(ir, slide_id, notes.clone())
            }
            _ => Err(DocumentError::InvalidPatchOp {
                reason: "op not yet implemented in HtmlOpApplier".into(),
                op: serde_json::to_value(op).unwrap_or_default(),
            }),
        }
    }

    fn add_slide(
        &self,
        ir: &mut HtmlIR,
        layout: SlideLayout,
        at_index: Option<u32>,
        title: Option<String>,
        subtitle: Option<String>,
    ) -> Result<OpOutcome, DocumentError> {
        if matches!(layout, SlideLayout::Title | SlideLayout::SectionDivider)
            && title.as_deref().unwrap_or("").trim().is_empty()
        {
            return Err(DocumentError::IRValidationFailed {
                path: "/op/add_slide".into(),
                reason: format!("layout {:?} requires non-empty title", layout),
            });
        }
        let id = self.ids.new_slide_id();
        let new_slide = Slide {
            id: id.clone(),
            layout,
            title,
            subtitle,
            notes: None,
            blocks: vec![],
        };
        let idx = at_index
            .map(|i| (i as usize).min(ir.slides.len()))
            .unwrap_or(ir.slides.len());
        ir.slides.insert(idx, new_slide);
        Ok(OpOutcome {
            assigned_ids: AssignedIds {
                slide: Some(id),
                ..Default::default()
            },
        })
    }

    fn delete_slide(&self, ir: &mut HtmlIR, slide_id: &str) -> Result<OpOutcome, DocumentError> {
        if ir.slides.len() <= 1 {
            return Err(DocumentError::IRValidationFailed {
                path: "/slides".into(),
                reason: "cannot delete last slide".into(),
            });
        }
        let pos = ir.slides.iter().position(|s| s.id == slide_id);
        match pos {
            Some(i) => {
                ir.slides.remove(i);
                recompute_assets_referenced(ir);
                Ok(OpOutcome::default())
            }
            None => Err(DocumentError::IRValidationFailed {
                path: format!("/slides/{slide_id}"),
                reason: "slide not found".into(),
            }),
        }
    }

    fn reorder_slides(
        &self,
        ir: &mut HtmlIR,
        order: &[String],
    ) -> Result<OpOutcome, DocumentError> {
        if order.len() != ir.slides.len() {
            return Err(DocumentError::IRValidationFailed {
                path: "/op/reorder_slides".into(),
                reason: format!(
                    "order length {} != current slides {}",
                    order.len(),
                    ir.slides.len()
                ),
            });
        }
        let mut new_slides = Vec::with_capacity(order.len());
        for id in order {
            let pos = ir.slides.iter().position(|s| s.id == *id).ok_or_else(|| {
                DocumentError::IRValidationFailed {
                    path: "/op/reorder_slides".into(),
                    reason: format!("unknown slide_id {id}"),
                }
            })?;
            new_slides.push(ir.slides.remove(pos));
        }
        ir.slides = new_slides;
        Ok(OpOutcome::default())
    }

    fn set_slide_layout(
        &self,
        ir: &mut HtmlIR,
        slide_id: &str,
        layout: SlideLayout,
    ) -> Result<OpOutcome, DocumentError> {
        let slide = find_slide_mut(ir, slide_id)?;
        if matches!(layout, SlideLayout::Title | SlideLayout::SectionDivider)
            && slide.title.as_deref().unwrap_or("").trim().is_empty()
        {
            return Err(DocumentError::IRValidationFailed {
                path: format!("/slides/{slide_id}"),
                reason: format!("layout {:?} requires non-empty title", layout),
            });
        }
        slide.layout = layout;
        Ok(OpOutcome::default())
    }

    fn set_slide_title(
        &self,
        ir: &mut HtmlIR,
        slide_id: &str,
        title: Option<String>,
        subtitle: Option<String>,
    ) -> Result<OpOutcome, DocumentError> {
        let slide = find_slide_mut(ir, slide_id)?;
        slide.title = title;
        slide.subtitle = subtitle;
        Ok(OpOutcome::default())
    }

    fn set_slide_notes(
        &self,
        ir: &mut HtmlIR,
        slide_id: &str,
        notes: Option<String>,
    ) -> Result<OpOutcome, DocumentError> {
        let slide = find_slide_mut(ir, slide_id)?;
        slide.notes = notes;
        Ok(OpOutcome::default())
    }
}

fn find_slide_mut<'b>(ir: &'b mut HtmlIR, id: &str) -> Result<&'b mut Slide, DocumentError> {
    ir.slides
        .iter_mut()
        .find(|s| s.id == id)
        .ok_or_else(|| DocumentError::IRValidationFailed {
            path: format!("/slides/{id}"),
            reason: "slide not found".into(),
        })
}

pub(crate) fn recompute_assets_referenced(ir: &mut HtmlIR) {
    let mut refs = std::collections::HashSet::new();
    for slide in &ir.slides {
        walk_block_assets(&slide.blocks, &mut refs);
    }
    let mut v: Vec<String> = refs.into_iter().collect();
    v.sort();
    ir.assets_referenced = v;
}

fn walk_block_assets(blocks: &[Block], out: &mut std::collections::HashSet<String>) {
    for b in blocks {
        match b {
            Block::Image {
                src: ImageSrc::Asset { asset_id },
                ..
            }
            | Block::Video {
                src: VideoSrc::Asset { asset_id },
                ..
            } => {
                out.insert(asset_id.clone());
            }
            Block::TwoColumns { left, right, .. } => {
                walk_block_assets(left, out);
                walk_block_assets(right, out);
            }
            Block::ThreeColumns {
                left,
                middle,
                right,
                ..
            } => {
                walk_block_assets(left, out);
                walk_block_assets(middle, out);
                walk_block_assets(right, out);
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::documents::domain::ir::html::{DocProps, FooterConfig, LayoutMode, Locale, Theme};
    use crate::documents::infrastructure::ids::CountingIdGenerator;

    fn fresh() -> HtmlIR {
        HtmlIR {
            kind: "html".into(),
            artifact_id: "art_x".into(),
            version_id: "v1".into(),
            schema_version: "1.0.0".into(),
            doc_props: DocProps {
                title: None,
                author: None,
                date: None,
                locale: Locale::En,
            },
            theme: Theme::Executive,
            layout_mode: LayoutMode::Slides,
            footer: FooterConfig {
                enabled: false,
                page_numbers: false,
                custom_text: None,
            },
            slides: vec![Slide {
                id: "sl_existing".into(),
                layout: SlideLayout::Blank,
                title: None,
                subtitle: None,
                notes: None,
                blocks: vec![],
            }],
            assets_referenced: vec![],
        }
    }

    #[test]
    fn add_slide_appends_and_returns_id() {
        let mut ir = fresh();
        let ids = CountingIdGenerator::default();
        let applier = HtmlOpApplier { ids: &ids };
        let out = applier
            .apply(
                &mut ir,
                &PatchOp::AddSlide {
                    layout: SlideLayout::Content,
                    at_index: None,
                    title: Some("New".into()),
                    subtitle: None,
                },
            )
            .unwrap();
        assert_eq!(ir.slides.len(), 2);
        assert_eq!(out.assigned_ids.slide.as_deref(), Some("sl_01"));
    }

    #[test]
    fn add_title_slide_without_title_rejected() {
        let mut ir = fresh();
        let ids = CountingIdGenerator::default();
        let applier = HtmlOpApplier { ids: &ids };
        let err = applier
            .apply(
                &mut ir,
                &PatchOp::AddSlide {
                    layout: SlideLayout::Title,
                    at_index: None,
                    title: Some("   ".into()),
                    subtitle: None,
                },
            )
            .unwrap_err();
        assert!(matches!(err, DocumentError::IRValidationFailed { .. }));
    }

    #[test]
    fn delete_last_slide_rejected() {
        let mut ir = fresh();
        let ids = CountingIdGenerator::default();
        let applier = HtmlOpApplier { ids: &ids };
        let err = applier
            .apply(
                &mut ir,
                &PatchOp::DeleteSlide {
                    slide_id: "sl_existing".into(),
                },
            )
            .unwrap_err();
        assert!(err.to_string().contains("cannot delete last"));
    }

    #[test]
    fn reorder_validates_length_and_ids() {
        let mut ir = fresh();
        ir.slides.push(Slide {
            id: "sl_b".into(),
            layout: SlideLayout::Blank,
            title: None,
            subtitle: None,
            notes: None,
            blocks: vec![],
        });
        let ids = CountingIdGenerator::default();
        let applier = HtmlOpApplier { ids: &ids };
        applier
            .apply(
                &mut ir,
                &PatchOp::ReorderSlides {
                    order: vec!["sl_b".into(), "sl_existing".into()],
                },
            )
            .unwrap();
        assert_eq!(ir.slides[0].id, "sl_b");
    }
}
