//! A://Labs API routes — Courses, Lessons, Enrollments, Certifications, Articles.
//!
//! Mirrors the Next.js `/api/v1` ALABS layer. OpenMAIC lesson generation
//! is implemented as direct LLM calls via the provider system.

use axum::extract::Extension;
use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::{delete, get, post},
    Json, Router,
};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;
use tracing::{info, warn};
use uuid::Uuid;

use crate::auth::AuthUser;
use crate::gizzi_completion;
use crate::AppState;

fn db_error(e: impl std::fmt::Display) -> impl IntoResponse {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({"error": "Database error", "details": e.to_string()})),
    )
}

pub fn alabs_router() -> Router<Arc<AppState>> {
    Router::new()
        // Courses
        .route("/courses", get(list_courses))
        .route("/courses/:id", get(get_course))
        // Lessons
        .route("/lessons", get(list_lessons).post(create_lesson))
        .route("/lessons/:id", delete(delete_lesson))
        .route("/lessons/generate", post(generate_lesson))
        // Enrollments
        .route(
            "/enrollments",
            get(list_enrollments).post(upsert_enrollment),
        )
        // Certifications
        .route(
            "/certifications",
            get(list_certifications).post(create_certification),
        )
        // Articles
        .route("/articles", get(list_articles).post(create_article))
        .route("/articles/:slug", get(get_article))
        // Capabilities
        .route(
            "/capabilities",
            get(list_capabilities).post(create_capability),
        )
}

// ═══════════════════════════════════════════════════════════════════════════════
// Courses
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Serialize)]
struct CourseRow {
    id: String,
    code: String,
    title: String,
    description: String,
    tier: String,
    canvas_url: Option<String>,
    modules: i64,
    capstone: String,
    cover_image: String,
    demos_url: Option<String>,
    sort_order: i64,
    published: bool,
    created_at: String,
    updated_at: String,
}

async fn list_courses(
    State(state): State<Arc<AppState>>,
    Extension(_user): Extension<AuthUser>,
    _headers: HeaderMap,
) -> impl IntoResponse {
    // Courses are public — no auth required
    let db = state.db.clone();

    let rows = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        let mut stmt = conn.prepare(
            "SELECT id, code, title, description, tier, canvas_url, modules, capstone, cover_image, demos_url, sort_order, published, created_at, updated_at
             FROM alabs_courses WHERE published = 1 ORDER BY sort_order"
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(CourseRow {
                id: row.get(0)?,
                code: row.get(1)?,
                title: row.get(2)?,
                description: row.get(3)?,
                tier: row.get(4)?,
                canvas_url: row.get(5)?,
                modules: row.get(6)?,
                capstone: row.get(7)?,
                cover_image: row.get(8)?,
                demos_url: row.get(9)?,
                sort_order: row.get(10)?,
                published: row.get::<_, i64>(11)? != 0,
                created_at: row.get(12)?,
                updated_at: row.get(13)?,
            })
        })?.collect::<Result<Vec<_>, _>>()?;
        Ok::<_, rusqlite::Error>(rows)
    }).await;

    match rows {
        Ok(Ok(rows)) => (StatusCode::OK, Json(json!(rows))).into_response(),
        Ok(Err(e)) => db_error(e).into_response(),
        Err(e) => db_error(e).into_response(),
    }
}

async fn get_course(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let db = state.db.clone();

    let row = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        let mut stmt = conn.prepare(
            "SELECT id, code, title, description, tier, canvas_url, modules, capstone, cover_image, demos_url, sort_order, published, created_at, updated_at
             FROM alabs_courses WHERE id = ?1"
        )?;
        let row = stmt.query_row([&id], |row| {
            Ok(CourseRow {
                id: row.get(0)?,
                code: row.get(1)?,
                title: row.get(2)?,
                description: row.get(3)?,
                tier: row.get(4)?,
                canvas_url: row.get(5)?,
                modules: row.get(6)?,
                capstone: row.get(7)?,
                cover_image: row.get(8)?,
                demos_url: row.get(9)?,
                sort_order: row.get(10)?,
                published: row.get::<_, i64>(11)? != 0,
                created_at: row.get(12)?,
                updated_at: row.get(13)?,
            })
        })?;
        Ok::<_, rusqlite::Error>(row)
    }).await;

    match row {
        Ok(Ok(row)) => (StatusCode::OK, Json(json!(row))).into_response(),
        Ok(Err(rusqlite::Error::QueryReturnedNoRows)) => (
            StatusCode::NOT_FOUND,
            Json(json!({"error": "Course not found"})),
        )
            .into_response(),
        Ok(Err(e)) => db_error(e).into_response(),
        Err(e) => db_error(e).into_response(),
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Lessons
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Serialize)]
struct LessonRow {
    id: String,
    course_id: String,
    module_number: i64,
    lesson_number: i64,
    title: String,
    description: String,
    content_markdown: Option<String>,
    content_html: Option<String>,
    scene_json: Option<String>,
    video_url: Option<String>,
    duration_minutes: i64,
    status: String,
    created_at: String,
    updated_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    course_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    course_title: Option<String>,
}

#[derive(Deserialize)]
struct ListLessonsQuery {
    course_id: Option<String>,
    status: Option<String>,
}

async fn list_lessons(
    State(state): State<Arc<AppState>>,
    Query(q): Query<ListLessonsQuery>,
) -> impl IntoResponse {
    let db = state.db.clone();

    let rows = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        let sql = if q.course_id.is_some() {
            "SELECT l.id, l.course_id, l.module_number, l.lesson_number, l.title, l.description,
                    l.content_markdown, l.content_html, l.scene_json, l.video_url, l.duration_minutes, l.status,
                    l.created_at, l.updated_at, c.code, c.title
             FROM alabs_lessons l
             JOIN alabs_courses c ON l.course_id = c.id
             WHERE l.status = ?1 AND l.course_id = ?2
             ORDER BY l.module_number, l.lesson_number"
        } else {
            "SELECT l.id, l.course_id, l.module_number, l.lesson_number, l.title, l.description,
                    l.content_markdown, l.content_html, l.scene_json, l.video_url, l.duration_minutes, l.status,
                    l.created_at, l.updated_at, c.code, c.title
             FROM alabs_lessons l
             JOIN alabs_courses c ON l.course_id = c.id
             WHERE l.status = ?1
             ORDER BY l.module_number, l.lesson_number"
        };

        let status = q.status.unwrap_or_else(|| "published".to_string());
        let mut stmt = conn.prepare(sql)?;

        let rows = if let Some(course_id) = q.course_id {
            stmt.query_map(params![&status, &course_id], map_lesson_row)?.collect::<Result<Vec<_>, _>>()?
        } else {
            stmt.query_map(params![&status], map_lesson_row)?.collect::<Result<Vec<_>, _>>()?
        };
        Ok::<_, rusqlite::Error>(rows)
    }).await;

    match rows {
        Ok(Ok(rows)) => (StatusCode::OK, Json(json!(rows))).into_response(),
        Ok(Err(e)) => db_error(e).into_response(),
        Err(e) => db_error(e).into_response(),
    }
}

fn map_lesson_row(row: &rusqlite::Row) -> Result<LessonRow, rusqlite::Error> {
    Ok(LessonRow {
        id: row.get(0)?,
        course_id: row.get(1)?,
        module_number: row.get(2)?,
        lesson_number: row.get(3)?,
        title: row.get(4)?,
        description: row.get(5)?,
        content_markdown: row.get(6)?,
        content_html: row.get(7)?,
        scene_json: row.get(8)?,
        video_url: row.get(9)?,
        duration_minutes: row.get(10)?,
        status: row.get(11)?,
        created_at: row.get(12)?,
        updated_at: row.get(13)?,
        course_code: row.get(14).ok(),
        course_title: row.get(15).ok(),
    })
}

#[derive(Deserialize)]
struct CreateLessonBody {
    course_id: String,
    module_number: Option<i64>,
    lesson_number: Option<i64>,
    title: String,
    description: Option<String>,
    content_markdown: Option<String>,
    scene_json: Option<String>,
    video_url: Option<String>,
    duration_minutes: Option<i64>,
}

async fn create_lesson(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    _headers: HeaderMap,
    Json(body): Json<CreateLessonBody>,
) -> impl IntoResponse {
    let db = state.db.clone();
    let id = format!(
        "lesson_{}",
        Uuid::new_v4()
            .to_string()
            .replace("-", "")
            .get(0..16)
            .unwrap_or("")
    );
    let now = chrono::Utc::now().to_rfc3339();

    let result = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        conn.execute(
            "INSERT INTO alabs_lessons (id, course_id, module_number, lesson_number, title, description, content_markdown, scene_json, video_url, duration_minutes, status, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 'published', ?11, ?12)",
            params![
                &id, &body.course_id,
                body.module_number.unwrap_or(1),
                body.lesson_number.unwrap_or(1),
                &body.title,
                body.description.as_deref().unwrap_or(""),
                body.content_markdown.as_deref(),
                body.scene_json.as_deref(),
                body.video_url.as_deref(),
                body.duration_minutes.unwrap_or(5),
                &now, &now,
            ],
        )?;
        Ok::<_, rusqlite::Error>(id)
    }).await;

    match result {
        Ok(Ok(id)) => {
            info!("Lesson created by {}: {}", user.user_id, id);
            (StatusCode::CREATED, Json(json!({"id": id}))).into_response()
        }
        Ok(Err(e)) => db_error(e).into_response(),
        Err(e) => db_error(e).into_response(),
    }
}

async fn delete_lesson(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    _headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let db = state.db.clone();
    let id_for_db = id.clone();
    let result = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        conn.execute(
            "UPDATE alabs_lessons SET status = 'archived', updated_at = ?1 WHERE id = ?2",
            params![chrono::Utc::now().to_rfc3339(), &id_for_db],
        )?;
        Ok::<_, rusqlite::Error>(())
    })
    .await;

    match result {
        Ok(Ok(())) => {
            info!("Lesson archived by {}: {}", user.user_id, id);
            (StatusCode::OK, Json(json!({"status": "archived"}))).into_response()
        }
        Ok(Err(e)) => db_error(e).into_response(),
        Err(e) => db_error(e).into_response(),
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// OpenMAIC — Lesson Generation via direct LLM calls
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Deserialize)]
struct GenerateLessonBody {
    course_id: String,
    topic: String,
    tier: Option<String>,
    module_number: Option<i64>,
    lesson_number: Option<i64>,
}

#[derive(Serialize, Clone)]
struct GeneratedLessonResponse {
    id: String,
    course_id: String,
    title: String,
    description: String,
    scene_json: String,
    content_markdown: String,
    duration_minutes: i64,
}

async fn generate_lesson(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    _headers: HeaderMap,
    Json(body): Json<GenerateLessonBody>,
) -> impl IntoResponse {
    // Fetch course details
    let db = state.db.clone();
    let course_id = body.course_id.clone();
    let course = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        let row = conn.query_row(
            "SELECT code, title, description, tier FROM alabs_courses WHERE id = ?1",
            [&course_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            },
        )?;
        Ok::<_, rusqlite::Error>(row)
    })
    .await;

    let (_course_code, course_title, course_desc, tier) = match course {
        Ok(Ok(c)) => c,
        _ => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({"error": "Course not found"})),
            )
                .into_response()
        }
    };

    // Build LLM prompt for lesson generation
    let tier = body.tier.clone().unwrap_or(tier);
    let prompt = build_lesson_prompt(&body.topic, &course_title, &course_desc, &tier);

    // Route lesson generation through the Gizzi runtime instead of calling
    // Anthropic directly. This ensures ALabs uses the same brain/provider
    // configuration as the rest of the platform.
    let system = "You are an expert curriculum designer for the Allternit A://Labs learning platform. You create structured lesson content with slides and quizzes. Output ONLY valid JSON matching the requested schema.";
    let ledger = crate::usage_ledger::LedgerCtx::surface("lessons");
    let lesson = match crate::usage_ledger::scope(ledger, gizzi_completion::complete(&prompt, Some(system), None)).await {
        Some(content) if !content.is_empty() => parse_llm_lesson(&content, &body, &course_title),
        _ => {
            warn!("Gizzi completion unavailable — falling back to rule-based generation");
            generate_fallback_lesson(&body, &course_title)
        }
    };

    persist_generated_lesson(state, user.user_id, body, lesson).await
}

fn build_lesson_prompt(topic: &str, course_title: &str, course_desc: &str, tier: &str) -> String {
    format!(
        r#"Create a lesson for the course "{course_title}" (tier: {tier}).
Course description: {course_desc}
Lesson topic: {topic}

Generate a JSON object with this exact structure:
{{
  "title": "Lesson title (max 60 chars)",
  "description": "1-2 sentence lesson summary",
  "duration_minutes": <integer 5-30>,
  "scenes": [
    {{ "type": "slide", "title": "Scene title", "content": "Markdown content. Use ## for subheadings, bullet points, code blocks.", "duration": <integer> }},
    {{ "type": "slide", "title": "...", "content": "...", "duration": <integer> }},
    {{ "type": "quiz", "title": "Knowledge Check", "questions": [
      {{ "question": "...", "options": ["A", "B", "C", "D"], "correctIndex": 0, "explanation": "Why this is correct" }}
    ], "duration": <integer> }},
    {{ "type": "slide", "title": "Summary", "content": "Key takeaways.", "duration": <integer> }}
  ]
}}

Rules:
- 4-6 scenes total
- First scene is an intro slide
- Include at least 1 quiz with 3-4 multiple-choice questions
- Content must be contextually relevant to: {topic}
- For tier {tier}, adjust depth: CORE = beginner, OPS = intermediate, AGENTS = advanced, ADV = expert
- All content in English
- Output ONLY the JSON object, no markdown fences, no commentary"#,
        course_title = course_title,
        course_desc = course_desc,
        tier = tier,
        topic = topic,
    )
}

fn parse_llm_lesson(
    text: &str,
    body: &GenerateLessonBody,
    course_title: &str,
) -> GeneratedLessonResponse {
    // Try to extract JSON from the response
    let json_str = if let Some(start) = text.find('{') {
        if let Some(end) = text.rfind('}') {
            &text[start..=end]
        } else {
            text
        }
    } else {
        text
    };

    match serde_json::from_str::<serde_json::Value>(json_str) {
        Ok(val) => {
            let title = val["title"].as_str().unwrap_or(&body.topic).to_string();
            let description = val["description"].as_str().unwrap_or("").to_string();
            let duration = val["duration_minutes"].as_i64().unwrap_or(10);
            let scenes = val["scenes"].clone();

            let scene_json = serde_json::to_string(&serde_json::json!({
                "version": "1.0",
                "scenes": scenes,
            }))
            .unwrap_or_else(|_| generate_fallback_scenes(&body.topic));

            let content_markdown = scenes
                .as_array()
                .map(|arr| {
                    arr.iter()
                        .map(|s| {
                            let t = s["title"].as_str().unwrap_or("");
                            let c = s["content"].as_str().unwrap_or("");
                            format!("## {}\n\n{}\n\n", t, c)
                        })
                        .collect::<String>()
                })
                .unwrap_or_default();

            GeneratedLessonResponse {
                id: format!(
                    "lesson_{}",
                    Uuid::new_v4()
                        .to_string()
                        .replace("-", "")
                        .get(0..16)
                        .unwrap_or("")
                ),
                course_id: body.course_id.clone(),
                title,
                description,
                scene_json,
                content_markdown,
                duration_minutes: duration,
            }
        }
        Err(e) => {
            warn!("Failed to parse LLM JSON: {} — falling back", e);
            generate_fallback_lesson(body, course_title)
        }
    }
}

fn generate_fallback_lesson(
    body: &GenerateLessonBody,
    course_title: &str,
) -> GeneratedLessonResponse {
    let scenes = serde_json::json!({
        "version": "1.0",
        "scenes": [
            {
                "type": "slide",
                "title": format!("Introduction to {}", body.topic),
                "content": format!("Welcome to this lesson on **{}** as part of **{}**.\n\nIn this lesson, you will:\n- Understand core concepts\n- Explore practical applications\n- Test your knowledge with a quiz", body.topic, course_title),
                "duration": 5
            },
            {
                "type": "slide",
                "title": "Key Concepts",
                "content": format!("## What is {}?\n\n{} is a fundamental topic in this course. Understanding it will help you build more effective systems and workflows.\n\n## Core Principles\n- Principle 1: Start with the fundamentals\n- Principle 2: Practice with real-world examples\n- Principle 3: Iterate and refine your approach", body.topic, body.topic),
                "duration": 8
            },
            {
                "type": "slide",
                "title": "Practical Application",
                "content": "## Putting It Into Practice\n\nConsider how you would apply these concepts to your current projects:\n\n1. Identify the problem space\n2. Map concepts to your domain\n3. Implement a small proof-of-concept\n4. Gather feedback and iterate",
                "duration": 7
            },
            {
                "type": "quiz",
                "title": "Knowledge Check",
                "questions": [
                    {
                        "question": format!("What is the primary focus of {}?", body.topic),
                        "options": ["Understanding core fundamentals", "Advanced optimization techniques", "Deployment strategies", "Team management"],
                        "correctIndex": 0,
                        "explanation": format!("{} focuses on building a strong foundation in core concepts before moving to advanced topics.", body.topic)
                    },
                    {
                        "question": "Which approach is recommended for learning new concepts?",
                        "options": ["Memorize all documentation", "Start with fundamentals and practice", "Skip to advanced topics", "Copy existing solutions"],
                        "correctIndex": 1,
                        "explanation": "Starting with fundamentals and practicing with real examples is the most effective learning approach."
                    }
                ],
                "duration": 10
            },
            {
                "type": "slide",
                "title": "Summary",
                "content": format!("## Key Takeaways\n\n- {} is essential for mastering this course\n- Start with fundamentals, then apply practically\n- Use the quiz to test your understanding\n- Continue to the next lesson to build on these concepts\n\n**Next Steps:** Review the concepts and try applying them to a real project.", body.topic),
                "duration": 5
            }
        ]
    });

    GeneratedLessonResponse {
        id: format!("lesson_{}", Uuid::new_v4().to_string().replace("-", "").get(0..16).unwrap_or("")),
        course_id: body.course_id.clone(),
        title: format!("{} — {}", course_title, body.topic),
        description: format!("An introduction to {} within the context of {}.", body.topic, course_title),
        scene_json: scenes.to_string(),
        content_markdown: format!("## Introduction to {}\n\nWelcome...\n\n## Key Concepts\n\n...\n\n## Practical Application\n\n...\n\n## Summary\n\n...", body.topic),
        duration_minutes: 35,
    }
}

fn generate_fallback_scenes(topic: &str) -> String {
    serde_json::json!({
        "version": "1.0",
        "scenes": [
            { "type": "slide", "title": format!("Introduction to {}", topic), "content": format!("Welcome to **{}**.", topic), "duration": 5 },
            { "type": "quiz", "title": "Knowledge Check", "questions": [{ "question": "What is this lesson about?", "options": [topic, "Other"], "correctIndex": 0, "explanation": "This lesson covers the selected topic." }], "duration": 5 }
        ]
    }).to_string()
}

async fn persist_generated_lesson(
    state: Arc<AppState>,
    user_id: String,
    body: GenerateLessonBody,
    lesson: GeneratedLessonResponse,
) -> axum::response::Response {
    let db = state.db.clone();
    let now = chrono::Utc::now().to_rfc3339();
    let lesson_id = lesson.id.clone();
    let module_number = body.module_number.unwrap_or(1);
    let lesson_number = body.lesson_number.unwrap_or(1);

    let lesson_response = lesson.clone();
    let result = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        conn.execute(
            "INSERT INTO alabs_lessons (id, course_id, module_number, lesson_number, title, description, content_markdown, scene_json, duration_minutes, status, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 'published', ?10, ?11)",
            params![
                &lesson.id, &lesson.course_id,
                module_number,
                lesson_number,
                &lesson.title, &lesson.description,
                &lesson.content_markdown, &lesson.scene_json,
                lesson.duration_minutes,
                &now, &now,
            ],
        )?;
        Ok::<_, rusqlite::Error>(())
    }).await;

    match result {
        Ok(Ok(())) => {
            info!("Generated lesson persisted by {}: {}", user_id, lesson_id);
            (StatusCode::CREATED, Json(json!(lesson_response))).into_response()
        }
        Ok(Err(e)) => db_error(e).into_response(),
        Err(e) => db_error(e).into_response(),
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Enrollments
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Serialize)]
struct EnrollmentRow {
    id: String,
    user_id: String,
    course_id: String,
    lesson_id: Option<String>,
    progress: i64,
    status: String,
    completed_at: Option<String>,
    created_at: String,
    updated_at: String,
}

#[derive(Deserialize)]
struct ListEnrollmentsQuery {
    course_id: Option<String>,
    lesson_id: Option<String>,
}

#[derive(Deserialize)]
struct UpsertEnrollmentBody {
    course_id: String,
    lesson_id: Option<String>,
    progress: Option<i64>,
    status: Option<String>,
}

async fn list_enrollments(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    _headers: HeaderMap,
    Query(q): Query<ListEnrollmentsQuery>,
) -> impl IntoResponse {
    let db = state.db.clone();
    let user_id = user.user_id;

    let rows = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        let mut sql = String::from(
            "SELECT id, user_id, course_id, lesson_id, progress, status, completed_at, created_at, updated_at
             FROM alabs_enrollments WHERE user_id = ?1"
        );
        if q.course_id.is_some() { sql.push_str(" AND course_id = ?2"); }
        if q.lesson_id.is_some() { sql.push_str(" AND lesson_id = ?3"); }

        let mut stmt = conn.prepare(&sql)?;
        let rows = match (&q.course_id, &q.lesson_id) {
            (Some(c), Some(l)) => stmt.query_map(params![&user_id, c, l], map_enrollment_row)?,
            (Some(c), None) => stmt.query_map(params![&user_id, c], map_enrollment_row)?,
            (None, Some(l)) => stmt.query_map(params![&user_id, l], map_enrollment_row)?,
            (None, None) => stmt.query_map(params![&user_id], map_enrollment_row)?,
        }.collect::<Result<Vec<_>, _>>()?;
        Ok::<_, rusqlite::Error>(rows)
    }).await;

    match rows {
        Ok(Ok(rows)) => (StatusCode::OK, Json(json!(rows))).into_response(),
        Ok(Err(e)) => db_error(e).into_response(),
        Err(e) => db_error(e).into_response(),
    }
}

fn map_enrollment_row(row: &rusqlite::Row) -> Result<EnrollmentRow, rusqlite::Error> {
    Ok(EnrollmentRow {
        id: row.get(0)?,
        user_id: row.get(1)?,
        course_id: row.get(2)?,
        lesson_id: row.get(3)?,
        progress: row.get(4)?,
        status: row.get(5)?,
        completed_at: row.get(6).ok(),
        created_at: row.get(7)?,
        updated_at: row.get(8)?,
    })
}

async fn upsert_enrollment(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    _headers: HeaderMap,
    Json(body): Json<UpsertEnrollmentBody>,
) -> impl IntoResponse {
    let db = state.db.clone();
    let user_id = user.user_id;
    let now = chrono::Utc::now().to_rfc3339();

    let result = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        let lesson_clause = if body.lesson_id.is_some() { "AND lesson_id = ?3" } else { "AND lesson_id IS NULL" };
        let sql = format!(
            "SELECT id, progress, status FROM alabs_enrollments WHERE user_id = ?1 AND course_id = ?2 {}",
            lesson_clause
        );

        let existing = if let Some(ref lesson_id) = body.lesson_id {
            conn.query_row(&sql, params![&user_id, &body.course_id, lesson_id], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?, row.get::<_, String>(2)?))
            }).ok()
        } else {
            conn.query_row(&sql, params![&user_id, &body.course_id], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?, row.get::<_, String>(2)?))
            }).ok()
        };

        if let Some((id, _old_progress, _old_status)) = existing {
            let progress = body.progress.unwrap_or(0);
            let status = body.status.as_deref().unwrap_or("in_progress");
            let completed_at = if status == "completed" { Some(&now) } else { None };
            conn.execute(
                "UPDATE alabs_enrollments SET progress = ?1, status = ?2, completed_at = ?3, updated_at = ?4 WHERE id = ?5",
                params![progress, status, completed_at, &now, &id],
            )?;
            Ok::<_, rusqlite::Error>(id)
        } else {
            let id = format!("enr_{}", Uuid::new_v4().to_string().replace("-", "").get(0..16).unwrap_or(""));
            let progress = body.progress.unwrap_or(0);
            let status = body.status.as_deref().unwrap_or("in_progress");
            let completed_at = if status == "completed" { Some(&now) } else { None };
            conn.execute(
                "INSERT INTO alabs_enrollments (id, user_id, course_id, lesson_id, progress, status, completed_at, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![&id, &user_id, &body.course_id, body.lesson_id.as_deref(), progress, status, completed_at, &now, &now],
            )?;
            Ok::<_, rusqlite::Error>(id)
        }
    }).await;

    match result {
        Ok(Ok(id)) => (StatusCode::OK, Json(json!({"id": id}))).into_response(),
        Ok(Err(e)) => db_error(e).into_response(),
        Err(e) => db_error(e).into_response(),
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Certifications
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Serialize)]
struct CertificationRow {
    id: String,
    user_id: String,
    course_code: String,
    course_title: String,
    tier: String,
    completed_at: Option<String>,
    capstone_url: Option<String>,
    score: Option<i64>,
    verified: bool,
    created_at: String,
    updated_at: String,
}

async fn list_certifications(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    _headers: HeaderMap,
) -> impl IntoResponse {
    let db = state.db.clone();
    let user_id = user.user_id;

    let rows = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        let mut stmt = conn.prepare(
            "SELECT id, user_id, course_code, course_title, tier, completed_at, capstone_url, score, verified, created_at, updated_at
             FROM alabs_certifications WHERE user_id = ?1 ORDER BY created_at DESC"
        )?;
        let rows = stmt.query_map([&user_id], |row| {
            Ok(CertificationRow {
                id: row.get(0)?,
                user_id: row.get(1)?,
                course_code: row.get(2)?,
                course_title: row.get(3)?,
                tier: row.get(4)?,
                completed_at: row.get(5).ok(),
                capstone_url: row.get(6).ok(),
                score: row.get(7).ok(),
                verified: row.get::<_, i64>(8)? != 0,
                created_at: row.get(9)?,
                updated_at: row.get(10)?,
            })
        })?.collect::<Result<Vec<_>, _>>()?;
        Ok::<_, rusqlite::Error>(rows)
    }).await;

    match rows {
        Ok(Ok(rows)) => (StatusCode::OK, Json(json!(rows))).into_response(),
        Ok(Err(e)) => db_error(e).into_response(),
        Err(e) => db_error(e).into_response(),
    }
}

#[derive(Deserialize)]
struct CreateCertificationBody {
    course_code: String,
    course_title: String,
    tier: String,
    capstone_url: Option<String>,
    score: Option<i64>,
}

async fn create_certification(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    _headers: HeaderMap,
    Json(body): Json<CreateCertificationBody>,
) -> impl IntoResponse {
    let db = state.db.clone();
    let id = format!(
        "cert_{}",
        Uuid::new_v4()
            .to_string()
            .replace("-", "")
            .get(0..16)
            .unwrap_or("")
    );
    let now = chrono::Utc::now().to_rfc3339();

    let result = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        conn.execute(
            "INSERT INTO alabs_certifications (id, user_id, course_code, course_title, tier, completed_at, capstone_url, score, verified, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 0, ?9, ?10)",
            params![
                &id, &user.user_id, &body.course_code, &body.course_title, &body.tier,
                &now, body.capstone_url.as_deref(), body.score,
                &now, &now,
            ],
        )?;
        Ok::<_, rusqlite::Error>(id)
    }).await;

    match result {
        Ok(Ok(id)) => (StatusCode::CREATED, Json(json!({"id": id}))).into_response(),
        Ok(Err(e)) => db_error(e).into_response(),
        Err(e) => db_error(e).into_response(),
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Articles
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Serialize)]
struct ArticleRow {
    id: String,
    slug: String,
    #[serde(rename = "type")]
    type_field: String,
    content_type: String,
    status: String,
    title: String,
    subtitle: String,
    abstract_text: String,
    authors: String,
    tags: String,
    keywords: String,
    content_markdown: Option<String>,
    reading_time: i64,
    featured: bool,
    series: Option<String>,
    issue_number: Option<String>,
    license: String,
    access_level: String,
    published_at: Option<String>,
    created_at: String,
    updated_at: String,
}

#[derive(Deserialize)]
struct ListArticlesQuery {
    status: Option<String>,
    content_type: Option<String>,
    featured: Option<bool>,
}

async fn list_articles(
    State(state): State<Arc<AppState>>,
    Query(q): Query<ListArticlesQuery>,
) -> impl IntoResponse {
    let db = state.db.clone();

    let rows = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        let mut sql = String::from(
            "SELECT id, slug, type, content_type, status, title, subtitle, abstract, authors, tags, keywords,
                    content_markdown, reading_time, featured, series, issue_number, license, access_level, published_at, created_at, updated_at
             FROM alabs_articles WHERE 1=1"
        );
        if q.status.is_some() { sql.push_str(" AND status = ?1"); }
        if q.content_type.is_some() { sql.push_str(" AND content_type = ?2"); }
        if q.featured.is_some() { sql.push_str(" AND featured = ?3"); }
        sql.push_str(" ORDER BY published_at DESC, created_at DESC");

        let mut stmt = conn.prepare(&sql)?;
        let featured_i64 = q.featured.map(|b| if b { 1 } else { 0 });
        let rows = stmt.query_map(
            params![q.status, q.content_type, featured_i64],
            |row| {
                Ok(ArticleRow {
                    id: row.get(0)?,
                    slug: row.get(1)?,
                    type_field: row.get(2)?,
                    content_type: row.get(3)?,
                    status: row.get(4)?,
                    title: row.get(5)?,
                    subtitle: row.get(6)?,
                    abstract_text: row.get(7)?,
                    authors: row.get(8)?,
                    tags: row.get(9)?,
                    keywords: row.get(10)?,
                    content_markdown: row.get(11).ok(),
                    reading_time: row.get(12)?,
                    featured: row.get::<_, i64>(13)? != 0,
                    series: row.get(14).ok(),
                    issue_number: row.get(15).ok(),
                    license: row.get(16)?,
                    access_level: row.get(17)?,
                    published_at: row.get(18).ok(),
                    created_at: row.get(19)?,
                    updated_at: row.get(20)?,
                })
            }
        )?.collect::<Result<Vec<_>, _>>()?;
        Ok::<_, rusqlite::Error>(rows)
    }).await;

    match rows {
        Ok(Ok(rows)) => (StatusCode::OK, Json(json!(rows))).into_response(),
        Ok(Err(e)) => db_error(e).into_response(),
        Err(e) => db_error(e).into_response(),
    }
}

#[derive(Deserialize)]
struct CreateArticleBody {
    slug: String,
    #[serde(rename = "type")]
    type_field: Option<String>,
    content_type: Option<String>,
    status: Option<String>,
    title: String,
    subtitle: Option<String>,
    #[serde(rename = "abstract")]
    abstract_text: Option<String>,
    content_markdown: Option<String>,
    content_html: Option<String>,
    reading_time: Option<i64>,
    featured: Option<bool>,
    series: Option<String>,
    issue_number: Option<String>,
    license: Option<String>,
    access_level: Option<String>,
    published_at: Option<String>,
}

async fn create_article(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    _headers: HeaderMap,
    Json(body): Json<CreateArticleBody>,
) -> impl IntoResponse {
    let db = state.db.clone();
    let id = format!(
        "art_{}",
        Uuid::new_v4()
            .to_string()
            .replace("-", "")
            .get(0..16)
            .unwrap_or("")
    );
    let now = chrono::Utc::now().to_rfc3339();

    let result = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        conn.execute(
            "INSERT INTO alabs_articles (id, slug, type, content_type, status, title, subtitle, abstract, authors, tags, keywords, content_markdown, content_html, reading_time, featured, series, issue_number, license, access_level, published_at, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, '[]', '[]', '[]', ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19)",
            params![
                &id, &body.slug,
                body.type_field.as_deref().unwrap_or("blog"),
                body.content_type.as_deref().unwrap_or("feature"),
                body.status.as_deref().unwrap_or("draft"),
                &body.title,
                body.subtitle.as_deref().unwrap_or(""),
                body.abstract_text.as_deref().unwrap_or(""),
                body.content_markdown.as_deref(),
                body.content_html.as_deref(),
                body.reading_time.unwrap_or(0),
                if body.featured.unwrap_or(false) { 1 } else { 0 },
                body.series.as_deref(),
                body.issue_number.as_deref(),
                body.license.as_deref().unwrap_or("CC BY 4.0"),
                body.access_level.as_deref().unwrap_or("public"),
                body.published_at.as_deref().unwrap_or(&now),
                &now, &now,
            ],
        )?;
        Ok::<_, rusqlite::Error>(id)
    }).await;

    match result {
        Ok(Ok(id)) => {
            info!("Article created by {}: {}", user.user_id, id);
            (StatusCode::CREATED, Json(json!({"id": id}))).into_response()
        }
        Ok(Err(e)) => db_error(e).into_response(),
        Err(e) => db_error(e).into_response(),
    }
}

async fn get_article(
    State(state): State<Arc<AppState>>,
    Path(slug): Path<String>,
) -> impl IntoResponse {
    let db = state.db.clone();

    let row = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        let mut stmt = conn.prepare(
            "SELECT id, slug, type, content_type, status, title, subtitle, abstract, authors, tags, keywords,
                    content_markdown, reading_time, featured, series, issue_number, license, access_level, published_at, created_at, updated_at
             FROM alabs_articles WHERE slug = ?1"
        )?;
        let row = stmt.query_row([&slug], |row| {
            Ok(ArticleRow {
                id: row.get(0)?,
                slug: row.get(1)?,
                type_field: row.get(2)?,
                content_type: row.get(3)?,
                status: row.get(4)?,
                title: row.get(5)?,
                subtitle: row.get(6)?,
                abstract_text: row.get(7)?,
                authors: row.get(8)?,
                tags: row.get(9)?,
                keywords: row.get(10)?,
                content_markdown: row.get(11).ok(),
                reading_time: row.get(12)?,
                featured: row.get::<_, i64>(13)? != 0,
                series: row.get(14).ok(),
                issue_number: row.get(15).ok(),
                license: row.get(16)?,
                access_level: row.get(17)?,
                published_at: row.get(18).ok(),
                created_at: row.get(19)?,
                updated_at: row.get(20)?,
            })
        })?;
        Ok::<_, rusqlite::Error>(row)
    }).await;

    match row {
        Ok(Ok(row)) => (StatusCode::OK, Json(json!(row))).into_response(),
        Ok(Err(rusqlite::Error::QueryReturnedNoRows)) => (
            StatusCode::NOT_FOUND,
            Json(json!({"error": "Article not found"})),
        )
            .into_response(),
        Ok(Err(e)) => db_error(e).into_response(),
        Err(e) => db_error(e).into_response(),
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Capabilities
// ═══════════════════════════════════════════════════════════════════════════════

async fn list_capabilities(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let db = state.db.clone();
    let rows = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        let mut stmt = conn.prepare("SELECT id, name, description, category, status, created_at FROM capabilities LIMIT 100")?;
        let rows = stmt.query_map([], |row| {
            Ok(json!({
                "id": row.get::<_, String>(0)?,
                "name": row.get::<_, String>(1)?,
                "description": row.get::<_, Option<String>>(2)?,
                "category": row.get::<_, Option<String>>(3)?,
                "status": row.get::<_, String>(4)?,
                "created_at": row.get::<_, String>(5)?,
            }))
        })?;
        let rows: Result<Vec<_>, _> = rows.collect();
        rows
    }).await;

    match rows {
        Ok(Ok(rows)) => Json(json!({ "capabilities": rows, "total": rows.len() })).into_response(),
        Ok(Err(e)) => db_error(e).into_response(),
        Err(e) => db_error(e).into_response(),
    }
}

async fn create_capability(
    State(state): State<Arc<AppState>>,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    let db = state.db.clone();
    let name = body
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let category = body
        .get("category")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let description = body
        .get("description")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let row = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        conn.execute(
            "INSERT INTO capabilities (id, name, description, category, status, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, 'active', CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)
             ON CONFLICT(id) DO UPDATE SET name=excluded.name, description=excluded.description, category=excluded.category, updated_at=CURRENT_TIMESTAMP",
            rusqlite::params![uuid::Uuid::new_v4().to_string(), name, description, category],
        )?;
        Ok::<_, rusqlite::Error>(())
    }).await;

    match row {
        Ok(Ok(())) => (StatusCode::CREATED, Json(json!({ "status": "created" }))).into_response(),
        Ok(Err(e)) => db_error(e).into_response(),
        Err(e) => db_error(e).into_response(),
    }
}
