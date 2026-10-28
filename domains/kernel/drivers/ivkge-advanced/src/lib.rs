//! IVKGE Advanced - Visual Knowledge Graph Extraction
//!
//! Implements advanced visual features:
//! - Screenshot parsing
//! - OCR integration
//! - User correction tools
//! - Natural image support
//! - Ambiguity controls
//!
//! See: P4.6 DAG Task Specification

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use thiserror::Error;
use tokio::sync::RwLock;

pub mod ambiguity;
pub mod corrections;
pub mod ocr;
pub mod screenshot;

pub use ambiguity::*;
pub use corrections::*;
pub use ocr::*;
pub use screenshot::*;

/// Extracted entity from visual input
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtractedEntity {
    pub entity_id: String,
    pub name: String,
    pub entity_type: String,
    pub confidence: f32,
    pub bounding_box: Option<BoundingBox>,
    pub properties: HashMap<String, String>,
}

/// Extracted relationship from visual input
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtractedRelationship {
    pub relationship_id: String,
    pub source_entity: String,
    pub target_entity: String,
    pub relationship_type: String,
    pub confidence: f32,
    pub label: Option<String>,
}

/// Bounding box for visual elements
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BoundingBox {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

/// Visual extraction result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VisualExtractionResult {
    pub extraction_id: String,
    pub source_type: VisualSourceType,
    pub entities: Vec<ExtractedEntity>,
    pub relationships: Vec<ExtractedRelationship>,
    pub ambiguity_report: Option<AmbiguityReport>,
    pub ocr_text: Option<String>,
    pub created_at: DateTime<Utc>,
}

/// Source type for visual extraction
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum VisualSourceType {
    Screenshot,
    Diagram,
    NaturalImage,
    Sketch,
}

/// IVKGE Advanced engine
pub struct IvkgeAdvancedEngine {
    screenshot_parser: ScreenshotParser,
    ocr_engine: OcrEngine,
    correction_manager: CorrectionManager,
    ambiguity_detector: AmbiguityDetector,
    extractions: Arc<RwLock<HashMap<String, VisualExtractionResult>>>,
}

impl IvkgeAdvancedEngine {
    /// Create a new IVKGE Advanced engine
    pub fn new() -> Self {
        Self {
            screenshot_parser: ScreenshotParser::new(),
            ocr_engine: OcrEngine::new(),
            correction_manager: CorrectionManager::new(),
            ambiguity_detector: AmbiguityDetector::new(),
            extractions: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Process a screenshot and extract entities/relationships
    pub async fn process_screenshot(
        &self,
        image_data: &[u8],
    ) -> Result<VisualExtractionResult, IvkgeError> {
        // Parse screenshot
        let parsed = self.screenshot_parser.parse(image_data)?;

        // Run OCR
        let ocr_result = self.ocr_engine.extract_text(image_data)?;

        // Detect ambiguities
        let ambiguity_report = self
            .ambiguity_detector
            .detect(&parsed.entities, &parsed.relationships);

        let extraction_id = format!("extract_{}", uuid::Uuid::new_v4().simple());

        let result = VisualExtractionResult {
            extraction_id: extraction_id.clone(),
            source_type: VisualSourceType::Screenshot,
            entities: parsed.entities,
            relationships: parsed.relationships,
            ambiguity_report,
            ocr_text: Some(ocr_result.text),
            created_at: Utc::now(),
        };

        // Store extraction
        {
            let mut extractions = self.extractions.write().await;
            extractions.insert(extraction_id, result.clone());
        }

        Ok(result)
    }

    /// Process a natural image
    pub async fn process_natural_image(
        &self,
        image_data: &[u8],
    ) -> Result<VisualExtractionResult, IvkgeError> {
        // Natural image processing with enhanced detection
        let parsed = self.screenshot_parser.parse_natural(image_data)?;

        // Run OCR
        let ocr_result = self.ocr_engine.extract_text(image_data)?;

        // Detect ambiguities (more common in natural images)
        let ambiguity_report = self
            .ambiguity_detector
            .detect(&parsed.entities, &parsed.relationships);

        let extraction_id = format!("extract_{}", uuid::Uuid::new_v4().simple());

        Ok(VisualExtractionResult {
            extraction_id,
            source_type: VisualSourceType::NaturalImage,
            entities: parsed.entities,
            relationships: parsed.relationships,
            ambiguity_report,
            ocr_text: Some(ocr_result.text),
            created_at: Utc::now(),
        })
    }

    /// Apply user correction to extraction
    pub async fn apply_correction(
        &self,
        extraction_id: &str,
        correction: UserCorrection,
    ) -> Result<VisualExtractionResult, IvkgeError> {
        self.correction_manager
            .apply(extraction_id, correction)
            .await
    }

    /// Get correction history for an extraction
    pub async fn get_correction_history(
        &self,
        extraction_id: &str,
    ) -> Vec<corrections::CorrectionRecord> {
        self.correction_manager.get_history(extraction_id).await
    }

    /// Resolve ambiguities based on user input
    pub async fn resolve_ambiguity(
        &self,
        extraction_id: &str,
        resolution: AmbiguityResolution,
    ) -> Result<VisualExtractionResult, IvkgeError> {
        self.ambiguity_detector
            .resolve(extraction_id, resolution)
            .await
    }

    /// Get all extractions
    pub async fn get_extractions(&self) -> Vec<VisualExtractionResult> {
        let extractions = self.extractions.read().await;
        extractions.values().cloned().collect()
    }

    /// Get specific extraction
    pub async fn get_extraction(&self, extraction_id: &str) -> Option<VisualExtractionResult> {
        let extractions = self.extractions.read().await;
        extractions.get(extraction_id).cloned()
    }
}

impl Default for IvkgeAdvancedEngine {
    fn default() -> Self {
        Self::new()
    }
}

/// IVKGE errors
#[derive(Debug, Error)]
pub enum IvkgeError {
    #[error("Image parsing failed: {0}")]
    ImageParse(String),

    #[error("OCR failed: {0}")]
    OcrFailed(String),

    #[error("Extraction not found: {0}")]
    ExtractionNotFound(String),

    #[error("Invalid correction: {0}")]
    InvalidCorrection(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bounding_box_creation() {
        let bbox = BoundingBox {
            x: 100,
            y: 200,
            width: 50,
            height: 30,
        };

        assert_eq!(bbox.x, 100);
        assert_eq!(bbox.y, 200);
        assert_eq!(bbox.width, 50);
        assert_eq!(bbox.height, 30);
    }

    #[test]
    fn test_extracted_entity_creation() {
        let mut properties = HashMap::new();
        properties.insert("color".to_string(), "blue".to_string());

        let entity = ExtractedEntity {
            entity_id: "entity_1".to_string(),
            name: "Test Entity".to_string(),
            entity_type: "component".to_string(),
            confidence: 0.95,
            bounding_box: Some(BoundingBox {
                x: 0,
                y: 0,
                width: 100,
                height: 50,
            }),
            properties,
        };

        assert_eq!(entity.entity_id, "entity_1");
        assert_eq!(entity.confidence, 0.95);
        assert!(entity.bounding_box.is_some());
    }

    #[test]
    fn test_extracted_relationship_creation() {
        let relationship = ExtractedRelationship {
            relationship_id: "rel_1".to_string(),
            source_entity: "entity_a".to_string(),
            target_entity: "entity_b".to_string(),
            relationship_type: "connects_to".to_string(),
            confidence: 0.88,
            label: Some("uses".to_string()),
        };

        assert_eq!(relationship.relationship_id, "rel_1");
        assert_eq!(relationship.source_entity, "entity_a");
        assert!(relationship.label.is_some());
    }

    #[test]
    fn test_visual_source_type_serialization() {
        let types = vec![
            VisualSourceType::Screenshot,
            VisualSourceType::Diagram,
            VisualSourceType::NaturalImage,
            VisualSourceType::Sketch,
        ];

        for source_type in types {
            let serialized = serde_json::to_string(&source_type).unwrap();
            assert!(!serialized.is_empty());
        }
    }

    #[test]
    fn test_visual_extraction_result_creation() {
        let result = VisualExtractionResult {
            extraction_id: "extract_test".to_string(),
            source_type: VisualSourceType::Screenshot,
            entities: vec![],
            relationships: vec![],
            ambiguity_report: None,
            ocr_text: Some("Test OCR".to_string()),
            created_at: Utc::now(),
        };

        assert_eq!(result.extraction_id, "extract_test");
        assert_eq!(result.source_type, VisualSourceType::Screenshot);
        assert!(result.ocr_text.is_some());
    }
}
