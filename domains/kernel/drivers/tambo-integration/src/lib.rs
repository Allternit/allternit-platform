//! Tambo Integration - UI Generation
//!
//! Implements UI generation from specifications:
//! - Canvas-based UI generation
//! - Component synthesis from specs
//! - Layout generation
//! - Style generation
//!
//! See: P4.11 DAG Task Specification

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use thiserror::Error;
use tokio::sync::RwLock;

pub mod a11y;
pub mod component_registry;
pub mod components;
pub mod generation_config;
pub mod generator;
pub mod hash;
pub mod layout;
pub mod schema_validator;
pub mod spec_diff;
pub mod streaming;
pub mod style;

pub use a11y::*;
pub use component_registry::*;
pub use components::*;
pub use generation_config::*;
pub use generator::*;
pub use hash::*;
pub use layout::*;
pub use schema_validator::*;
pub use spec_diff::*;
pub use streaming::*;
pub use style::*;

/// UI specification for generation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UISpec {
    pub spec_id: String,
    pub title: String,
    pub description: String,
    pub components: Vec<ComponentSpec>,
    pub layout: LayoutSpec,
    pub style: StyleSpec,
    pub interactions: Vec<InteractionSpec>,
    pub created_at: DateTime<Utc>,
}

/// Component template
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComponentTemplate {
    pub template_id: String,
    pub component_type: String,
    pub template_code: String,
    pub properties: Vec<String>,
    #[serde(default)]
    pub keywords: Vec<String>, // For deterministic component selection
}

/// Component specification
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComponentSpec {
    pub component_id: String,
    pub component_type: String,
    pub properties: HashMap<String, serde_json::Value>,
    pub children: Vec<String>,
    pub bindings: Vec<DataBinding>,
}

/// Data binding for component
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DataBinding {
    pub property: String,
    pub source: String,
    pub transform: Option<String>,
}

/// Layout specification
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LayoutSpec {
    pub layout_type: String,
    pub constraints: LayoutConstraints,
    pub regions: Vec<LayoutRegionSpec>,
}

/// Layout constraints
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LayoutConstraints {
    pub min_width: Option<u32>,
    pub max_width: Option<u32>,
    pub min_height: Option<u32>,
    pub max_height: Option<u32>,
}

/// Layout region specification
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LayoutRegionSpec {
    pub region_id: String,
    pub region_type: String,
    pub position: RegionPosition,
    pub size: RegionSize,
}

/// Region position
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegionPosition {
    pub x: f32,
    pub y: f32,
}

/// Region size
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegionSize {
    pub width: f32,
    pub height: f32,
}

/// Style specification
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StyleSpec {
    pub theme: String,
    pub colors: HashMap<String, String>,
    pub typography: TypographySpec,
    pub spacing: SpacingSpec,
}

/// Typography specification
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TypographySpec {
    pub font_family: String,
    pub font_sizes: HashMap<String, u32>,
    pub line_heights: HashMap<String, f32>,
}

/// Spacing specification
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpacingSpec {
    pub scale: Vec<u32>,
    pub unit: String,
}

/// Interaction specification
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InteractionSpec {
    pub interaction_id: String,
    pub trigger: String,
    pub action: String,
    pub target: String,
    pub parameters: HashMap<String, serde_json::Value>,
}

/// Generated UI output
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeneratedUI {
    pub generation_id: String,
    pub spec_id: String,
    pub ui_code: String,
    pub ui_type: UIType,
    pub components_generated: usize,
    pub confidence: f32,
    pub created_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generation_hash: Option<String>,
}

/// UI type
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum UIType {
    React,
    Vue,
    Svelte,
    Angular,
    WebComponents,
    PlainHtml,
}

/// Tambo UI generation engine
pub struct TamboEngine {
    generator: UIGenerator,
    layout_engine: LayoutEngine,
    style_engine: StyleEngine,
    component_library: ComponentLibrary,
    component_registry: ComponentRegistry,
    schema_validator: SchemaValidator,
    hash_engine: HashEngine,
    a11y_engine: A11yEngine,
    spec_diff_engine: SpecDiffEngine,
    specs: Arc<RwLock<HashMap<String, UISpec>>>,
    generations: Arc<RwLock<HashMap<String, GeneratedUI>>>,
    generation_states: Arc<RwLock<HashMap<String, GenerationState>>>,
}

/// Generation state for persistence
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenerationState {
    pub generation_id: String,
    pub spec_id: String,
    pub state: serde_json::Value,
    pub version: u32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl TamboEngine {
    /// Create a new Tambo engine
    pub fn new() -> Self {
        Self {
            generator: UIGenerator::new(),
            layout_engine: LayoutEngine::new(),
            style_engine: StyleEngine::new(),
            component_library: ComponentLibrary::new(),
            component_registry: ComponentRegistry::new(),
            schema_validator: SchemaValidator::new(),
            hash_engine: HashEngine::new(),
            a11y_engine: A11yEngine::new(),
            spec_diff_engine: SpecDiffEngine::new(),
            specs: Arc::new(RwLock::new(HashMap::new())),
            generations: Arc::new(RwLock::new(HashMap::new())),
            generation_states: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Initialize engine with default components
    pub async fn init_with_defaults() -> Self {
        let engine = Self::new();

        // Register default component templates
        let defaults = vec![
            ComponentTemplate {
                template_id: "btn-001".to_string(),
                component_type: "button".to_string(),
                template_code: r#"<button id="{{id}}" class="btn btn-primary">{{label}}</button>"#.to_string(),
                properties: vec!["id".to_string(), "label".to_string()],
                keywords: vec!["button".to_string(), "click".to_string(), "action".to_string()],
            },
            ComponentTemplate {
                template_id: "inp-001".to_string(),
                component_type: "input".to_string(),
                template_code: r#"<input id="{{id}}" type="{{type}}" placeholder="{{placeholder}}" />"#.to_string(),
                properties: vec!["id".to_string(), "type".to_string(), "placeholder".to_string()],
                keywords: vec!["input".to_string(), "text".to_string(), "form".to_string()],
            },
            ComponentTemplate {
                template_id: "card-001".to_string(),
                component_type: "card".to_string(),
                template_code: r#"<div id="{{id}}" class="card"><div class="card-header">{{title}}</div><div class="card-body">{{content}}</div></div>"#.to_string(),
                properties: vec!["id".to_string(), "title".to_string(), "content".to_string()],
                keywords: vec!["card".to_string(), "container".to_string(), "panel".to_string()],
            },
            ComponentTemplate {
                template_id: "cont-001".to_string(),
                component_type: "container".to_string(),
                template_code: r#"<div id="{{id}}" class="container">{{content}}</div>"#.to_string(),
                properties: vec!["id".to_string(), "content".to_string()],
                keywords: vec!["container".to_string(), "wrapper".to_string(), "div".to_string()],
            },
        ];

        for template in defaults {
            engine.register_component_template(template).await;
        }

        engine
    }

    /// Generate UI from specification
    pub async fn generate_ui(
        &self,
        spec: &UISpec,
        ui_type: UIType,
    ) -> Result<GeneratedUI, TamboError> {
        // Validate specification
        self.validate_spec(spec)?;

        // Generate layout
        let layout = self.layout_engine.generate(&spec.layout)?;

        // Generate styles
        let styles = self.style_engine.generate(&spec.style)?;

        // Generate components
        let mut components_code = Vec::new();
        for component_spec in &spec.components {
            let code =
                self.component_library
                    .generate_component(component_spec, &layout, &styles)?;
            components_code.push(code);
        }

        // Assemble final UI
        let ui_code = self
            .generator
            .assemble(&components_code, &layout, &styles, ui_type)?;

        let result = GeneratedUI {
            generation_id: format!("gen_{}", uuid::Uuid::new_v4().simple()),
            spec_id: spec.spec_id.clone(),
            ui_code,
            ui_type,
            components_generated: spec.components.len(),
            confidence: 0.9,
            created_at: Utc::now(),
            generation_hash: None,
        };

        // Store spec and generation
        {
            let mut specs = self.specs.write().await;
            specs.insert(spec.spec_id.clone(), spec.clone());
        }
        {
            let mut generations = self.generations.write().await;
            generations.insert(result.generation_id.clone(), result.clone());
        }

        Ok(result)
    }

    /// Validate UI specification
    fn validate_spec(&self, spec: &UISpec) -> Result<(), TamboError> {
        if spec.spec_id.is_empty() {
            return Err(TamboError::InvalidSpec("spec_id is required".to_string()));
        }

        if spec.components.is_empty() {
            return Err(TamboError::InvalidSpec(
                "At least one component is required".to_string(),
            ));
        }

        Ok(())
    }

    /// Generate UI from specification with schema validation
    pub async fn generate_ui_validated(
        &self,
        spec: &UISpec,
        ui_type: UIType,
    ) -> Result<GeneratedUI, TamboError> {
        // Validate spec against schema
        self.schema_validator
            .validate_spec(spec)
            .map_err(|e| TamboError::ValidationFailed(e.message))?;

        // Generate UI
        let result = self.generate_ui(spec, ui_type).await?;

        // Validate output against schema
        self.schema_validator
            .validate_output(&result)
            .map_err(|e| TamboError::ValidationFailed(e.message))?;

        Ok(result)
    }

    /// Generate UI from specification with reproducibility (seed-based)
    pub async fn generate_ui_reproducible(
        &self,
        spec: &UISpec,
        ui_type: UIType,
        config: GenerationConfig,
    ) -> Result<GeneratedUI, TamboError> {
        // Generate UI first
        let mut result = self.generate_ui(spec, ui_type).await?;

        // Make deterministic based on seed
        if let Some(seed) = config.seed {
            // Create deterministic generation ID from seed and spec
            result.generation_id = format!("gen_{:016x}_{}", seed, self.compute_spec_hash(spec));

            // Compute deterministic hash (excluding random generation_id)
            let hash = self.compute_deterministic_hash(&result, seed);
            result.generation_hash = Some(hash);
        } else {
            // Compute regular hash
            let hash = self.compute_generation_hash(&result);
            result.generation_hash = Some(hash);
        }

        Ok(result)
    }

    /// Hash a spec for deterministic ID generation (internal)
    fn compute_spec_hash(&self, spec: &UISpec) -> String {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let mut hasher = DefaultHasher::new();
        spec.spec_id.hash(&mut hasher);
        spec.title.hash(&mut hasher);
        spec.components.len().hash(&mut hasher);
        format!("{:08x}", hasher.finish())
    }

    /// Compute deterministic hash (seed-based)
    fn compute_deterministic_hash(&self, ui: &GeneratedUI, seed: u64) -> String {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let mut hasher = DefaultHasher::new();
        // Include seed for reproducibility
        seed.hash(&mut hasher);
        // Include spec_id
        ui.spec_id.hash(&mut hasher);
        // Include the actual content
        ui.ui_code.hash(&mut hasher);
        ui.components_generated.hash(&mut hasher);

        format!("{:016x}", hasher.finish())
    }

    /// Save generation state for persistence
    pub async fn save_generation_state(
        &self,
        generation_id: &str,
        spec_id: &str,
        state: serde_json::Value,
    ) -> Result<(), TamboError> {
        let mut states = self.generation_states.write().await;

        let entry = states
            .entry(generation_id.to_string())
            .or_insert_with(|| GenerationState {
                generation_id: generation_id.to_string(),
                spec_id: spec_id.to_string(),
                state: serde_json::json!({}),
                version: 0,
                created_at: Utc::now(),
                updated_at: Utc::now(),
            });

        entry.state = state;
        entry.version += 1;
        entry.updated_at = Utc::now();

        Ok(())
    }

    /// Load generation state
    pub async fn load_generation_state(
        &self,
        generation_id: &str,
    ) -> Result<Option<GenerationState>, TamboError> {
        let states = self.generation_states.read().await;
        Ok(states.get(generation_id).cloned())
    }

    /// Compute hash of a generation for verification
    fn compute_generation_hash(&self, ui: &GeneratedUI) -> String {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let mut hasher = DefaultHasher::new();
        ui.generation_id.hash(&mut hasher);
        ui.spec_id.hash(&mut hasher);
        ui.ui_code.hash(&mut hasher);
        ui.components_generated.hash(&mut hasher);

        format!("{:016x}", hasher.finish())
    }

    /// Get all specs
    pub async fn get_specs(&self) -> Vec<UISpec> {
        let specs = self.specs.read().await;
        specs.values().cloned().collect()
    }

    /// Get specific spec
    pub async fn get_spec(&self, spec_id: &str) -> Option<UISpec> {
        let specs = self.specs.read().await;
        specs.get(spec_id).cloned()
    }

    /// Create spec
    pub async fn create_spec(&self, spec: UISpec) {
        let mut specs = self.specs.write().await;
        specs.insert(spec.spec_id.clone(), spec);
    }

    /// Delete spec
    pub async fn delete_spec(&self, spec_id: &str) -> bool {
        let mut specs = self.specs.write().await;
        specs.remove(spec_id).is_some()
    }

    /// Get all generations
    pub async fn get_generations(&self) -> Vec<GeneratedUI> {
        let generations = self.generations.read().await;
        generations.values().cloned().collect()
    }

    /// Get specific generation
    pub async fn get_generation(&self, generation_id: &str) -> Option<GeneratedUI> {
        let generations = self.generations.read().await;
        generations.get(generation_id).cloned()
    }

    /// Get available component types
    pub async fn get_component_types(&self) -> Vec<String> {
        self.component_registry.get_component_types().await
    }

    /// Get all templates
    pub async fn get_templates(&self) -> Vec<ComponentTemplate> {
        self.component_registry.get_all_templates().await
    }

    /// Select components deterministically based on keywords
    pub async fn select_components(&self, keywords: &[String]) -> Vec<ComponentTemplate> {
        self.component_registry.select_components(keywords).await
    }

    /// Register a custom component template
    pub async fn register_component_template(&self, template: ComponentTemplate) {
        self.component_registry.register_template(template).await;
    }

    /// Register template (legacy alias)
    pub async fn register_template(&self, template: ComponentTemplate) {
        self.component_library.register_template(template).await
    }

    // ====================================================================
    // Hash & Verification Methods
    // ====================================================================

    /// Hash a UI spec for content verification
    pub fn hash_spec(&self, spec: &UISpec) -> String {
        self.hash_engine.hash_spec(spec)
    }

    /// Hash a component spec
    pub fn hash_component(&self, component: &ComponentSpec) -> String {
        self.hash_engine.hash_component(component)
    }

    /// Get hierarchical hash for a spec
    pub fn hierarchical_hash(&self, spec: &UISpec) -> HierarchicalHash {
        self.hash_engine.hierarchical_hash(spec)
    }

    /// Verify content matches expected hash
    pub fn verify_hash(&self, content: &str, expected_hash: &str) -> bool {
        self.hash_engine.verify_hash(content, expected_hash)
    }

    // ====================================================================
    // Accessibility Methods
    // ====================================================================

    /// Check accessibility compliance of a spec
    pub fn check_a11y(&self, spec: &UISpec) -> A11yCheckResult {
        self.a11y_engine.check_spec(spec)
    }

    /// Check accessibility of generated UI
    pub fn check_generated_ui_a11y(&self, ui: &GeneratedUI) -> A11yCheckResult {
        self.a11y_engine.check_generated_ui(ui)
    }

    /// Generate ARIA attributes for a component
    pub fn generate_aria_attributes(&self, component: &ComponentSpec) -> Vec<(String, String)> {
        self.a11y_engine.generate_aria_attributes(component)
    }

    /// Generate accessibility report
    pub fn generate_a11y_report(&self, result: &A11yCheckResult) -> String {
        self.a11y_engine.generate_report(result)
    }

    /// Check color contrast
    pub fn check_contrast(&self, foreground: &str, background: &str) -> (bool, f32) {
        self.a11y_engine.check_contrast(foreground, background)
    }

    /// Generate accessibility hash
    pub fn generate_a11y_hash(&self, spec: &UISpec) -> String {
        crate::a11y::generate_a11y_hash(spec)
    }

    // ====================================================================
    // Engine Accessors
    // ====================================================================

    /// Get reference to hash engine
    pub fn hash_engine(&self) -> &HashEngine {
        &self.hash_engine
    }

    /// Get reference to accessibility engine
    pub fn a11y_engine(&self) -> &A11yEngine {
        &self.a11y_engine
    }

    /// Get reference to spec diff engine
    pub fn spec_diff_engine(&self) -> &SpecDiffEngine {
        &self.spec_diff_engine
    }
}

impl Default for TamboEngine {
    fn default() -> Self {
        Self::new()
    }
}

/// Tambo errors
#[derive(Debug, Error)]
pub enum TamboError {
    #[error("Invalid specification: {0}")]
    InvalidSpec(String),

    #[error("Validation failed: {0}")]
    ValidationFailed(String),

    #[error("Layout generation failed: {0}")]
    LayoutFailed(String),

    #[error("Style generation failed: {0}")]
    StyleFailed(String),

    #[error("Component generation failed: {0}")]
    ComponentFailed(String),

    #[error("Assembly failed: {0}")]
    AssemblyFailed(String),

    #[error("Unknown component type: {0}")]
    UnknownComponent(String),
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ui_spec_creation() {
        let mut properties = HashMap::new();
        properties.insert("label".to_string(), serde_json::json!("Click Me"));

        let spec = UISpec {
            spec_id: "spec_1".to_string(),
            title: "Test UI".to_string(),
            description: "A test UI specification".to_string(),
            components: vec![ComponentSpec {
                component_id: "btn_1".to_string(),
                component_type: "button".to_string(),
                properties,
                children: vec![],
                bindings: vec![],
            }],
            layout: LayoutSpec {
                layout_type: "flex".to_string(),
                constraints: LayoutConstraints::default(),
                regions: vec![],
            },
            style: StyleSpec {
                theme: "light".to_string(),
                colors: HashMap::new(),
                typography: TypographySpec {
                    font_family: "Arial".to_string(),
                    font_sizes: HashMap::new(),
                    line_heights: HashMap::new(),
                },
                spacing: SpacingSpec {
                    scale: vec![4, 8, 16, 32],
                    unit: "px".to_string(),
                },
            },
            interactions: vec![],
            created_at: Utc::now(),
        };

        assert_eq!(spec.spec_id, "spec_1");
        assert_eq!(spec.components.len(), 1);
        assert_eq!(spec.components[0].component_type, "button");
    }

    #[test]
    fn test_component_spec_creation() {
        let mut properties = HashMap::new();
        properties.insert("variant".to_string(), serde_json::json!("primary"));

        let component = ComponentSpec {
            component_id: "comp_1".to_string(),
            component_type: "card".to_string(),
            properties,
            children: vec!["child_1".to_string()],
            bindings: vec![DataBinding {
                property: "title".to_string(),
                source: "data.title".to_string(),
                transform: None,
            }],
        };

        assert_eq!(component.component_id, "comp_1");
        assert_eq!(component.component_type, "card");
        assert_eq!(component.children.len(), 1);
        assert_eq!(component.bindings.len(), 1);
    }

    #[test]
    fn test_layout_spec_creation() {
        let layout = LayoutSpec {
            layout_type: "grid".to_string(),
            constraints: LayoutConstraints {
                min_width: Some(320),
                max_width: Some(1920),
                min_height: None,
                max_height: None,
            },
            regions: vec![LayoutRegionSpec {
                region_id: "header".to_string(),
                region_type: "header".to_string(),
                position: RegionPosition { x: 0.0, y: 0.0 },
                size: RegionSize {
                    width: 100.0,
                    height: 10.0,
                },
            }],
        };

        assert_eq!(layout.layout_type, "grid");
        assert_eq!(layout.constraints.min_width, Some(320));
        assert_eq!(layout.regions.len(), 1);
    }

    #[test]
    fn test_style_spec_creation() {
        let mut colors = HashMap::new();
        colors.insert("primary".to_string(), "#3b82f6".to_string());

        let mut font_sizes = HashMap::new();
        font_sizes.insert("small".to_string(), 12);
        font_sizes.insert("medium".to_string(), 16);

        let style = StyleSpec {
            theme: "dark".to_string(),
            colors,
            typography: TypographySpec {
                font_family: "Inter".to_string(),
                font_sizes,
                line_heights: HashMap::new(),
            },
            spacing: SpacingSpec {
                scale: vec![4, 8, 16, 24, 32],
                unit: "rem".to_string(),
            },
        };

        assert_eq!(style.theme, "dark");
        assert_eq!(style.colors.get("primary"), Some(&"#3b82f6".to_string()));
        assert_eq!(style.typography.font_family, "Inter");
    }

    #[test]
    fn test_interaction_spec_creation() {
        let mut parameters = HashMap::new();
        parameters.insert("delay".to_string(), serde_json::json!(500));

        let interaction = InteractionSpec {
            interaction_id: "int_1".to_string(),
            trigger: "click".to_string(),
            action: "navigate".to_string(),
            target: "/dashboard".to_string(),
            parameters,
        };

        assert_eq!(interaction.interaction_id, "int_1");
        assert_eq!(interaction.trigger, "click");
        assert_eq!(interaction.action, "navigate");
    }

    #[test]
    fn test_ui_type_serialization() {
        let types = vec![
            UIType::React,
            UIType::Vue,
            UIType::Svelte,
            UIType::PlainHtml,
        ];

        for ui_type in types {
            let serialized = serde_json::to_string(&ui_type).unwrap();
            assert!(!serialized.is_empty());
        }
    }

    #[test]
    fn test_generated_ui_creation() {
        let generated = GeneratedUI {
            generation_id: "gen_test".to_string(),
            spec_id: "spec_test".to_string(),
            ui_code: "<div>Test</div>".to_string(),
            ui_type: UIType::React,
            components_generated: 5,
            confidence: 0.95,
            created_at: Utc::now(),
            generation_hash: None,
        };

        assert_eq!(generated.generation_id, "gen_test");
        assert_eq!(generated.ui_type, UIType::React);
        assert_eq!(generated.components_generated, 5);
        assert_eq!(generated.confidence, 0.95);
    }

    #[test]
    fn test_spec_validation_empty_id() {
        let engine = TamboEngine::new();

        let spec = UISpec {
            spec_id: "".to_string(),
            title: "Test".to_string(),
            description: "Test".to_string(),
            components: vec![],
            layout: LayoutSpec {
                layout_type: "flex".to_string(),
                constraints: LayoutConstraints::default(),
                regions: vec![],
            },
            style: StyleSpec {
                theme: "light".to_string(),
                colors: HashMap::new(),
                typography: TypographySpec {
                    font_family: "Arial".to_string(),
                    font_sizes: HashMap::new(),
                    line_heights: HashMap::new(),
                },
                spacing: SpacingSpec {
                    scale: vec![],
                    unit: "px".to_string(),
                },
            },
            interactions: vec![],
            created_at: Utc::now(),
        };

        let result = engine.validate_spec(&spec);
        assert!(result.is_err());
    }

    #[test]
    fn test_spec_validation_no_components() {
        let engine = TamboEngine::new();

        let spec = UISpec {
            spec_id: "spec_1".to_string(),
            title: "Test".to_string(),
            description: "Test".to_string(),
            components: vec![],
            layout: LayoutSpec {
                layout_type: "flex".to_string(),
                constraints: LayoutConstraints::default(),
                regions: vec![],
            },
            style: StyleSpec {
                theme: "light".to_string(),
                colors: HashMap::new(),
                typography: TypographySpec {
                    font_family: "Arial".to_string(),
                    font_sizes: HashMap::new(),
                    line_heights: HashMap::new(),
                },
                spacing: SpacingSpec {
                    scale: vec![],
                    unit: "px".to_string(),
                },
            },
            interactions: vec![],
            created_at: Utc::now(),
        };

        let result = engine.validate_spec(&spec);
        assert!(result.is_err());
    }
}
