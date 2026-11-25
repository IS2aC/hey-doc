//! The Hey-Doc chat for signed-in accounts.
//!
//! - A photo is read by Reducto and matched to the catalogue. The photo itself is not kept.
//! - A typed message that names a medicine with its strength ("Doliprane 1000 mg") is matched
//!   to the catalogue the same way, without the model.
//! - Any other message is answered by the language model with the system prompt in
//!   `prompts/chat-system.md`, given the last messages of the conversation.
//!
//! The answer is computed before anything is written: when the model fails, nothing is stored.

use axum::{
    Json,
    extract::{Multipart, Path, State, rejection::JsonRejection},
    http::{HeaderMap, StatusCode},
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{PgPool, types::Json as Jsonb};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::{
    ApiError, AppState, accounts,
    prescriptions::{self, MAX_IMAGE_BYTES, image_kind},
    reducto::Image,
};

const SYSTEM_PROMPT: &str = include_str!("../prompts/chat-system.md");
const MAX_TEXT: usize = 4000;
const HISTORY: i64 = 20;

#[derive(Serialize, sqlx::FromRow, ToSchema)]
pub struct ChatMessage {
    id: Uuid,
    /// `user` ou `assistant`.
    role: String,
    /// `text`, `photo` (photo envoyée, non conservée) ou `prescription` (lignes relevées, détaillées dans `data`).
    kind: String,
    content: String,
    /// Pour `prescription` : la lecture, au format de `POST /v1/prescriptions/match`.
    #[schema(value_type = Option<Object>)]
    data: Option<Jsonb<Value>>,
    created_at: DateTime<Utc>,
}

#[derive(Serialize, sqlx::FromRow, ToSchema)]
pub struct ConversationSummary {
    id: Uuid,
    title: String,
    updated_at: DateTime<Utc>,
}

#[derive(Serialize, ToSchema)]
pub struct Conversation {
    id: Uuid,
    title: String,
    messages: Vec<ChatMessage>,
}

#[derive(Serialize, ToSchema)]
pub struct Exchange {
    conversation: ConversationSummary,
    /// Le message envoyé, puis la réponse de Hey-Doc.
    messages: Vec<ChatMessage>,
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(example = json!({"conversation_id": null, "text": "Comment trouver une pharmacie de garde ce soir ?"}))]
pub struct SendRequest {
    /// Absent pour commencer une nouvelle conversation.
    conversation_id: Option<Uuid>,
    /// De 1 à 4 000 caractères.
    text: String,
}

struct Reply {
    kind: &'static str,
    content: String,
    data: Option<Value>,
}

fn title_from(text: &str) -> String {
    let one_line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut t: String = one_line.chars().take(60).collect();
    if one_line.chars().count() > 60 {
        t.push('…');
    }
    t
}

/// The conversation must belong to the signed-in account.
async fn owned(pool: &PgPool, signup: Uuid, id: Uuid) -> Result<ConversationSummary, ApiError> {
    sqlx::query_as("SELECT id, title, updated_at FROM conversations WHERE id = $1 AND signup_id = $2")
        .bind(id)
        .bind(signup)
        .fetch_optional(pool)
        .await?
        .ok_or(ApiError::NotFound("conversation_not_found"))
}

/// Writes the exchange in one transaction, creating the conversation if needed.
async fn store(
    pool: &PgPool,
    signup: Uuid,
    conversation: Option<Uuid>,
    title: String,
    user: (&str, String, Option<Value>),
    reply: Reply,
) -> Result<Exchange, ApiError> {
    let mut tx = pool.begin().await?;
    let summary: ConversationSummary = match conversation {
        Some(id) => sqlx::query_as("UPDATE conversations SET updated_at = now() WHERE id = $1 RETURNING id, title, updated_at")
            .bind(id)
            .fetch_one(&mut *tx)
            .await?,
        None => sqlx::query_as("INSERT INTO conversations (id, signup_id, title) VALUES ($1, $2, $3) RETURNING id, title, updated_at")
            .bind(Uuid::new_v4())
            .bind(signup)
            .bind(title)
            .fetch_one(&mut *tx)
            .await?,
    };
    let mut messages = vec![];
    for (role, kind, content, data) in [("user", user.0, user.1, user.2), ("assistant", reply.kind, reply.content, reply.data)] {
        let m: ChatMessage = sqlx::query_as(
            "INSERT INTO messages (id, conversation_id, role, kind, content, data) VALUES ($1, $2, $3, $4, $5, $6)
             RETURNING id, role, kind, content, data, created_at",
        )
        .bind(Uuid::new_v4())
        .bind(summary.id)
        .bind(role)
        .bind(kind)
        .bind(content)
        .bind(data.map(Jsonb))
        .fetch_one(&mut *tx)
        .await?;
        messages.push(m);
    }
    tx.commit().await?;
    Ok(Exchange { conversation: summary, messages })
}

fn llm_error(e: crate::llm::LlmError) -> ApiError {
    let detail = match e {
        crate::llm::LlmError::Unavailable(d) | crate::llm::LlmError::InvalidAnswer(d) => d,
    };
    tracing::error!(%detail, "réponse du chat impossible");
    ApiError::Upstream("chat_failed", "Hey-Doc n’a pas pu répondre. Réessayez dans un moment.".into())
}

/// Envoyer un message.
///
/// Hey-Doc répond dans la même requête. Un message qui nomme un médicament avec son dosage est
/// rapproché du catalogue ; les autres reçoivent une réponse du modèle de langage.
#[utoipa::path(post, path = "/v1/chat/messages", tag = "chat", request_body = SendRequest,
    responses(
        (status = 201, body = Exchange),
        (status = 400, body = crate::ErrorBody),
        (status = 401, body = crate::ErrorBody),
        (status = 404, body = crate::ErrorBody),
        (status = 502, body = crate::ErrorBody, description = "Le modèle a échoué"),
        (status = 503, body = crate::ErrorBody, description = "Aucun modèle configuré")))]
pub async fn send(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Result<Json<SendRequest>, JsonRejection>,
) -> Result<(StatusCode, Json<Exchange>), ApiError> {
    let (signup, first_name) = accounts::current(&state.pool, &headers).await?;
    let Json(b) = body?;
    let text = b.text.trim().to_owned();
    if text.is_empty() || text.chars().count() > MAX_TEXT {
        return Err(ApiError::BadRequest("invalid_text", format!("text doit contenir de 1 à {MAX_TEXT} caractères")));
    }
    if let Some(id) = b.conversation_id {
        owned(&state.pool, signup, id).await?;
    }

    let mut reply = None;
    if prescriptions::looks_like_prescription(&text) {
        let read = prescriptions::match_typed(&state.pool, &text).await?;
        if read.line_count() > 0 {
            reply = Some(Reply { kind: "prescription", content: read.summary(), data: serde_json::to_value(&read).ok() });
        }
    }
    let reply = match reply {
        Some(r) => r,
        None => {
            let Some(llm) = state.llm.clone() else {
                return Err(ApiError::Unavailable("llm_unavailable", "Le chat n’est pas configuré (LLM_MODEL).".into()));
            };
            let mut history: Vec<(String, String)> = match b.conversation_id {
                Some(id) => sqlx::query_as(
                    "SELECT role, content FROM (SELECT role, content, created_at FROM messages WHERE conversation_id = $1
                     ORDER BY created_at DESC LIMIT $2) last ORDER BY created_at",
                )
                .bind(id)
                .bind(HISTORY)
                .fetch_all(&state.pool)
                .await?,
                None => vec![],
            };
            history.push(("user".into(), text.clone()));
            let system = SYSTEM_PROMPT
                .replace("{first_name}", &first_name)
                .replace("{today}", &Utc::now().format("%d/%m/%Y").to_string());
            let (content, usage) = llm.chat(&system, &history).await.map_err(llm_error)?;
            tracing::info!(model = %usage.model, prompt = ?usage.prompt_tokens, completion = ?usage.completion_tokens, "réponse du chat");
            Reply { kind: "text", content, data: None }
        }
    };
    let exchange = store(&state.pool, signup, b.conversation_id, title_from(&text), ("text", text, None), reply).await?;
    Ok((StatusCode::CREATED, Json(exchange)))
}

#[derive(ToSchema)]
#[allow(dead_code)]
pub struct PhotoUpload {
    /// Photo JPEG, PNG ou HEIC de l’ordonnance, 10 Mo au plus.
    #[schema(value_type = String, format = Binary)]
    image: Vec<u8>,
    /// Absent pour commencer une nouvelle conversation.
    conversation_id: Option<String>,
}

/// Envoyer une photo d’ordonnance.
///
/// La photo est lue par Reducto et rapprochée du catalogue. Elle n’est pas conservée : la
/// conversation garde seulement les lignes relevées.
#[utoipa::path(post, path = "/v1/chat/photo", tag = "chat",
    request_body(content = PhotoUpload, content_type = "multipart/form-data"),
    responses(
        (status = 201, body = Exchange),
        (status = 400, body = crate::ErrorBody),
        (status = 401, body = crate::ErrorBody),
        (status = 502, body = crate::ErrorBody),
        (status = 503, body = crate::ErrorBody)))]
pub async fn photo(State(state): State<AppState>, headers: HeaderMap, mut multipart: Multipart) -> Result<(StatusCode, Json<Exchange>), ApiError> {
    let (signup, _) = accounts::current(&state.pool, &headers).await?;
    let invalid = |msg: &str| ApiError::BadRequest("invalid_image", msg.into());
    let (mut image, mut conversation) = (None, None);
    while let Some(field) = multipart.next_field().await.map_err(|_| invalid("Formulaire invalide ou image de plus de 10 Mo"))? {
        match field.name() {
            Some("image") => {
                let bytes = field.bytes().await.map_err(|_| invalid("Image illisible ou de plus de 10 Mo"))?;
                if bytes.len() > MAX_IMAGE_BYTES {
                    return Err(invalid("L’image dépasse 10 Mo"));
                }
                let extension = image_kind(&bytes).ok_or_else(|| invalid("Formats acceptés : JPEG, PNG, HEIC"))?;
                image = Some(Image { bytes: bytes.to_vec(), extension });
            }
            Some("conversation_id") => {
                let v = field.text().await.unwrap_or_default();
                if !v.trim().is_empty() {
                    conversation = Some(v.trim().parse::<Uuid>().map_err(|_| ApiError::BadRequest("invalid_conversation", "conversation_id invalide".into()))?);
                }
            }
            _ => {}
        }
    }
    let image = image.ok_or_else(|| invalid("Champ « image » manquant"))?;
    if let Some(id) = conversation {
        owned(&state.pool, signup, id).await?;
    }
    let Some(reader) = state.reader.clone() else { return Err(prescriptions::no_reader()) };
    let read = prescriptions::read_image(reader.as_ref(), &state.pool, image).await?;
    let reply = Reply { kind: "prescription", content: read.summary(), data: serde_json::to_value(&read).ok() };
    let title = format!("Ordonnance du {}", Utc::now().format("%d/%m"));
    let exchange = store(&state.pool, signup, conversation, title, ("photo", "Photo d’ordonnance".into(), None), reply).await?;
    Ok((StatusCode::CREATED, Json(exchange)))
}

/// Mes conversations.
///
/// Les 50 conversations les plus récentes du compte connecté.
#[utoipa::path(get, path = "/v1/chat/conversations", tag = "chat",
    responses((status = 200, body = Vec<ConversationSummary>), (status = 401, body = crate::ErrorBody)))]
pub async fn list(State(pool): State<PgPool>, headers: HeaderMap) -> Result<Json<Vec<ConversationSummary>>, ApiError> {
    let (signup, _) = accounts::current(&pool, &headers).await?;
    let rows = sqlx::query_as("SELECT id, title, updated_at FROM conversations WHERE signup_id = $1 ORDER BY updated_at DESC LIMIT 50")
        .bind(signup)
        .fetch_all(&pool)
        .await?;
    Ok(Json(rows))
}

/// Une conversation et ses messages.
#[utoipa::path(get, path = "/v1/chat/conversations/{id}", tag = "chat",
    responses((status = 200, body = Conversation), (status = 401, body = crate::ErrorBody), (status = 404, body = crate::ErrorBody)))]
pub async fn get(State(pool): State<PgPool>, headers: HeaderMap, Path(id): Path<Uuid>) -> Result<Json<Conversation>, ApiError> {
    let (signup, _) = accounts::current(&pool, &headers).await?;
    let c = owned(&pool, signup, id).await?;
    let messages = sqlx::query_as(
        "SELECT id, role, kind, content, data, created_at FROM messages WHERE conversation_id = $1 ORDER BY created_at",
    )
    .bind(id)
    .fetch_all(&pool)
    .await?;
    Ok(Json(Conversation { id: c.id, title: c.title, messages }))
}

/// Supprimer une conversation.
#[utoipa::path(delete, path = "/v1/chat/conversations/{id}", tag = "chat",
    responses((status = 204), (status = 401, body = crate::ErrorBody), (status = 404, body = crate::ErrorBody)))]
pub async fn delete(State(pool): State<PgPool>, headers: HeaderMap, Path(id): Path<Uuid>) -> Result<StatusCode, ApiError> {
    let (signup, _) = accounts::current(&pool, &headers).await?;
    let done = sqlx::query("DELETE FROM conversations WHERE id = $1 AND signup_id = $2").bind(id).bind(signup).execute(&pool).await?;
    if done.rows_affected() == 0 {
        return Err(ApiError::NotFound("conversation_not_found"));
    }
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::title_from;

    #[test]
    fn titles_are_one_short_line() {
        assert_eq!(title_from("Bonjour\n  Hey-Doc"), "Bonjour Hey-Doc");
        assert_eq!(title_from(&"a".repeat(70)).chars().count(), 61);
    }
}
