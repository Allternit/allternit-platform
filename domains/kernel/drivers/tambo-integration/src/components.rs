//! Component Library Module
//!
//! Generates UI components from specifications.

use crate::{ComponentSpec, ComponentTemplate, Layout, Style, TamboError};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Component library
#[derive(Clone)]
pub struct ComponentLibrary {
    templates: Arc<RwLock<HashMap<String, String>>>,
}

impl ComponentLibrary {
    /// Create a new component library
    pub fn new() -> Self {
        let mut templates = HashMap::new();

        // Register default component templates
        templates.insert("button".to_string(), Self::button_template());
        templates.insert("input".to_string(), Self::input_template());
        templates.insert("card".to_string(), Self::card_template());
        templates.insert("container".to_string(), Self::container_template());
        templates.insert("table".to_string(), Self::table_template());
        templates.insert("modal".to_string(), Self::modal_template());
        templates.insert("tabs".to_string(), Self::tabs_template());
        templates.insert("dropdown".to_string(), Self::dropdown_template());
        templates.insert("chart".to_string(), Self::chart_template());

        Self {
            templates: Arc::new(RwLock::new(templates)),
        }
    }

    /// Generate a component from spec
    pub fn generate_component(
        &self,
        spec: &ComponentSpec,
        _layout: &Layout,
        _styles: &Style,
    ) -> Result<String, TamboError> {
        // Get template for component type
        let templates = self
            .templates
            .try_read()
            .map_err(|e| TamboError::ComponentFailed(format!("Lock error: {}", e)))?;

        let template = templates
            .get(&spec.component_type)
            .ok_or_else(|| TamboError::UnknownComponent(spec.component_type.clone()))?;

        // Generate component from template
        let mut code = template.clone();

        // Replace placeholders with actual values
        code = code.replace("{{component_id}}", &spec.component_id);

        // Replace properties
        for (key, value) in &spec.properties {
            let placeholder = format!("{{{{props.{}}}}}", key);
            let value_str = match value {
                serde_json::Value::String(s) => s.clone(),
                serde_json::Value::Number(n) => n.to_string(),
                serde_json::Value::Bool(b) => b.to_string(),
                _ => value.to_string(),
            };
            code = code.replace(&placeholder, &value_str);
        }

        Ok(code)
    }

    /// Register a custom template (legacy - use register_template with ComponentTemplate)
    pub async fn register_template_string(&self, component_type: &str, template: String) {
        let mut templates = self.templates.write().await;
        templates.insert(component_type.to_string(), template);
    }

    /// Get available component types
    pub async fn get_available_types(&self) -> Vec<String> {
        let templates = self.templates.read().await;
        templates.keys().cloned().collect()
    }

    /// Get all templates
    pub async fn get_templates(&self) -> Vec<ComponentTemplate> {
        let templates = self.templates.read().await;
        templates
            .iter()
            .map(|(comp_type, template)| ComponentTemplate {
                template_id: format!("tpl_{}", comp_type),
                component_type: comp_type.clone(),
                template_code: template.clone(),
                properties: vec!["id".to_string()],
                keywords: vec![],
            })
            .collect()
    }

    /// Register a template
    pub async fn register_template(&self, template: ComponentTemplate) {
        let mut templates = self.templates.write().await;
        templates.insert(template.component_type, template.template_code);
    }

    // Default component templates

    fn button_template() -> String {
        r#"<button id="{{component_id}}" class="btn btn-primary">{{props.label}}</button>"#
            .to_string()
    }

    fn input_template() -> String {
        r#"<input id="{{component_id}}" type="{{props.type}}" placeholder="{{props.placeholder}}" class="input" />"#.to_string()
    }

    fn card_template() -> String {
        r#"<div id="{{component_id}}" class="card">
  <div class="card-header">{{props.title|Card Title}}</div>
  <div class="card-body">{{props.content|Card content}}</div>
</div>"#
            .to_string()
    }

    fn container_template() -> String {
        r#"<div id="{{component_id}}" class="container {{props.variant|default}}">
  {{children}}
</div>"#
            .to_string()
    }

    fn table_template() -> String {
        r#"<table id="{{component_id}}" class="table {{props.class}}" role="table" aria-label="{{props.ariaLabel|Data table}}">
  <thead>
    <tr>
      <!-- Table headers will be generated based on data -->
      <th scope="col">Column 1</th>
      <th scope="col">Column 2</th>
      <th scope="col">Column 3</th>
    </tr>
  </thead>
  <tbody>
    <tr>
      <td>Data 1</td>
      <td>Data 2</td>
      <td>Data 3</td>
    </tr>
  </tbody>
</table>"#.to_string()
    }

    fn modal_template() -> String {
        r#"<div id="{{component_id}}" class="modal {{props.size}}" role="dialog" aria-modal="true" aria-labelledby="{{component_id}}-title">
  <div class="modal-overlay" onclick="closeModal('{{component_id}}')"></div>
  <div class="modal-content">
    <div class="modal-header">
      <h3 id="{{component_id}}-title">{{props.title|Modal Title}}</h3>
      <button class="modal-close" onclick="closeModal('{{component_id}}')" aria-label="Close">×</button>
    </div>
    <div class="modal-body">
      {{props.content|Modal content goes here}}
    </div>
    <div class="modal-footer">
      <button class="btn btn-secondary" onclick="closeModal('{{component_id}}')">{{props.cancelText|Cancel}}</button>
      <button class="btn btn-primary" onclick="confirmModal('{{component_id}}')">{{props.confirmText|Confirm}}</button>
    </div>
  </div>
</div>"#.to_string()
    }

    fn tabs_template() -> String {
        r#"<div id="{{component_id}}" class="tabs">
  <div class="tabs-list" role="tablist" aria-label="{{props.ariaLabel|Tab Navigation}}">
    <button class="tab-trigger active" role="tab" aria-selected="true" data-tab="0">
      {{props.tab1Label|Tab 1}}
    </button>
    <button class="tab-trigger" role="tab" aria-selected="false" data-tab="1">
      {{props.tab2Label|Tab 2}}
    </button>
    <button class="tab-trigger" role="tab" aria-selected="false" data-tab="2">
      {{props.tab3Label|Tab 3}}
    </button>
  </div>
  <div class="tabs-content">
    <div class="tab-panel active" role="tabpanel" data-tab-index="0">
      {{props.tab1Content|Content for tab 1}}
    </div>
    <div class="tab-panel" role="tabpanel" data-tab-index="1" hidden>
      {{props.tab2Content|Content for tab 2}}
    </div>
    <div class="tab-panel" role="tabpanel" data-tab-index="2" hidden>
      {{props.tab3Content|Content for tab 3}}
    </div>
  </div>
</div>"#
            .to_string()
    }

    fn dropdown_template() -> String {
        r#"<div id="{{component_id}}" class="dropdown">
  <button 
    id="{{component_id}}-trigger"
    class="dropdown-trigger" 
    onclick="toggleDropdown('{{component_id}}')"
    aria-haspopup="listbox"
    aria-expanded="false"
    aria-controls="{{component_id}}-menu"
  >
    <span class="dropdown-label">{{props.label|Select an option}}</span>
    <span class="dropdown-arrow" aria-hidden="true">▼</span>
  </button>
  <ul 
    id="{{component_id}}-menu"
    class="dropdown-menu" 
    role="listbox" 
    aria-labelledby="{{component_id}}-trigger"
    aria-label="{{props.label|Options}}"
  >
    <li class="dropdown-option" role="option" data-value="{{props.option1Value|1}}">{{props.option1Label|Option 1}}</li>
    <li class="dropdown-option" role="option" data-value="{{props.option2Value|2}}">{{props.option2Label|Option 2}}</li>
    <li class="dropdown-option" role="option" data-value="{{props.option3Value|3}}">{{props.option3Label|Option 3}}</li>
  </ul>
</div>"#.to_string()
    }

    fn chart_template() -> String {
        r#"<div id="{{component_id}}" class="chart-container" role="img" aria-label="{{props.ariaLabel|Chart showing data visualization}}">
  <div class="chart-header">
    <h4>{{props.title|Chart}}</h4>
    <p class="chart-subtitle">{{props.subtitle}}</p>
  </div>
  <div class="chart-body" data-chart-type="{{props.type|line}}">
    <!-- Chart renders here - data: {{props.data}} -->
    <svg class="chart-svg" viewBox="0 0 400 200" aria-hidden="true">
      <!-- Static chart representation -->
      <rect x="10" y="150" width="30" height="40" fill="blue" />
      <rect x="50" y="100" width="30" height="90" fill="blue" />
      <rect x="90" y="120" width="30" height="70" fill="blue" />
    </svg>
  </div>
</div>"#.to_string()
    }
}

impl Default for ComponentLibrary {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_get_available_types() {
        let library = ComponentLibrary::new();
        let types = library.get_available_types().await;

        assert!(types.contains(&"button".to_string()));
        assert!(types.contains(&"input".to_string()));
        assert!(types.contains(&"card".to_string()));
        assert!(types.contains(&"container".to_string()));
        assert!(types.contains(&"table".to_string()));
        assert!(types.contains(&"modal".to_string()));
        assert!(types.contains(&"tabs".to_string()));
        assert!(types.contains(&"dropdown".to_string()));
        assert!(types.contains(&"chart".to_string()));
    }

    #[tokio::test]
    async fn test_register_custom_template() {
        let library = ComponentLibrary::new();

        let custom_template =
            r#"<custom-component id="{{component_id}}">{{props.content}}</custom-component>"#
                .to_string();
        library
            .register_template_string("custom", custom_template)
            .await;

        let types = library.get_available_types().await;
        assert!(types.contains(&"custom".to_string()));
    }

    #[test]
    fn test_button_template() {
        let template = ComponentLibrary::button_template();
        assert!(template.contains("{{component_id}}"));
        assert!(template.contains("{{props.label}}"));
        assert!(template.contains("<button"));
    }

    #[test]
    fn test_input_template() {
        let template = ComponentLibrary::input_template();
        assert!(template.contains("{{component_id}}"));
        assert!(template.contains("{{props.type}}"));
        assert!(template.contains("<input"));
    }

    #[test]
    fn test_card_template() {
        let template = ComponentLibrary::card_template();
        assert!(template.contains("{{component_id}}"));
        assert!(template.contains("{{props.title|Card Title}}"));
        assert!(template.contains("<div"));
    }

    #[test]
    fn test_container_template() {
        let template = ComponentLibrary::container_template();
        assert!(template.contains("{{component_id}}"));
        assert!(template.contains("{{props.variant|default}}"));
        assert!(template.contains("{{children}}"));
    }

    #[tokio::test]
    async fn test_generate_button_component() {
        let library = ComponentLibrary::new();

        let mut properties = HashMap::new();
        properties.insert("label".to_string(), serde_json::json!("Submit"));

        let spec = ComponentSpec {
            component_id: "btn_submit".to_string(),
            component_type: "button".to_string(),
            properties,
            children: vec![],
            bindings: vec![],
        };

        let layout = Layout {
            container_class: "container".to_string(),
            regions: vec![],
        };

        let style = Style { css: String::new() };

        let result = library.generate_component(&spec, &layout, &style);
        assert!(result.is_ok());

        let code = result.unwrap();
        assert!(code.contains("btn_submit"));
        // Template uses {{props.label}} format - replaced when property is provided
        assert!(code.contains("<button"));
    }

    #[tokio::test]
    async fn test_generate_unknown_component() {
        let library = ComponentLibrary::new();

        let spec = ComponentSpec {
            component_id: "unknown_1".to_string(),
            component_type: "unknown_type".to_string(),
            properties: HashMap::new(),
            children: vec![],
            bindings: vec![],
        };

        let layout = Layout {
            container_class: "container".to_string(),
            regions: vec![],
        };

        let style = Style { css: String::new() };

        let result = library.generate_component(&spec, &layout, &style);
        assert!(result.is_err());
        assert!(matches!(result, Err(TamboError::UnknownComponent(_))));
    }

    #[test]
    fn test_table_template() {
        let template = ComponentLibrary::table_template();
        assert!(template.contains("{{component_id}}"));
        assert!(template.contains("<table"));
        assert!(template.contains("<thead"));
        assert!(template.contains("<tbody"));
    }

    #[test]
    fn test_modal_template() {
        let template = ComponentLibrary::modal_template();
        assert!(template.contains("{{component_id}}"));
        assert!(template.contains("role=\"dialog\""));
        assert!(template.contains("aria-modal=\"true\""));
        assert!(template.contains("modal-content"));
    }

    #[test]
    fn test_tabs_template() {
        let template = ComponentLibrary::tabs_template();
        assert!(template.contains("{{component_id}}"));
        assert!(template.contains("role=\"tablist\""));
        assert!(template.contains("role=\"tab\""));
        assert!(template.contains("role=\"tabpanel\""));
    }

    #[test]
    fn test_dropdown_template() {
        let template = ComponentLibrary::dropdown_template();
        assert!(template.contains("{{component_id}}"));
        assert!(template.contains("aria-haspopup=\"listbox\""));
        assert!(template.contains("role=\"listbox\""));
        assert!(template.contains("role=\"option\""));
    }

    #[test]
    fn test_chart_template() {
        let template = ComponentLibrary::chart_template();
        assert!(template.contains("{{component_id}}"));
        assert!(template.contains("chart-container"));
        assert!(template.contains("chart-svg"));
    }
}
