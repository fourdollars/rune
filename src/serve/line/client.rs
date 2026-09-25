use super::types::{
    LineGroupSummary, LineMessage, LineProfile, LoadingStartReq, PushMessageReq, ReplyMessageReq,
};
use reqwest::Client;
use tracing::{debug, error, info, warn};

/// LINE Messaging API client for interacting with the LINE platform.
#[derive(Clone)]
pub struct LineClient {
    channel_access_token: String,
    http_client: Client,
    base_url: String,
}

impl LineClient {
    /// Create a new LineClient with channel access token.
    pub fn new(channel_access_token: String) -> Self {
        Self {
            channel_access_token,
            http_client: Client::builder()
                .timeout(std::time::Duration::from_secs(15))
                .build()
                .unwrap_or_else(|_| Client::new()),
            base_url: "https://api.line.me".to_string(),
        }
    }

    /// Create with a custom base URL (useful for testing and mocks).
    pub fn new_with_base_url(channel_access_token: String, base_url: String) -> Self {
        Self {
            channel_access_token,
            http_client: Client::builder()
                .timeout(std::time::Duration::from_secs(15))
                .build()
                .unwrap_or_else(|_| Client::new()),
            base_url,
        }
    }

    /// Fetch user profile (displayName, pictureUrl, statusMessage) by userId.
    pub async fn get_profile(&self, user_id: &str) -> Result<LineProfile, anyhow::Error> {
        let url = format!("{}/v2/bot/profile/{}", self.base_url, user_id);
        let resp = self
            .http_client
            .get(&url)
            .bearer_auth(&self.channel_access_token)
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let err_text = resp.text().await.unwrap_or_default();
            anyhow::bail!("LINE get_profile failed [{}]: {}", status, err_text);
        }

        let profile: LineProfile = resp.json().await?;
        Ok(profile)
    }

    /// Fetch group summary (groupId, groupName) by groupId.
    pub async fn get_group_summary(
        &self,
        group_id: &str,
    ) -> Result<LineGroupSummary, anyhow::Error> {
        let url = format!("{}/v2/bot/group/{}/summary", self.base_url, group_id);
        let resp = self
            .http_client
            .get(&url)
            .bearer_auth(&self.channel_access_token)
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let err_text = resp.text().await.unwrap_or_default();
            anyhow::bail!("LINE get_group_summary failed [{}]: {}", status, err_text);
        }

        let summary: LineGroupSummary = resp.json().await?;
        Ok(summary)
    }

    /// Fetch group member profile (displayName, pictureUrl) by groupId and userId.
    pub async fn get_group_member_profile(
        &self,
        group_id: &str,
        user_id: &str,
    ) -> Result<LineProfile, anyhow::Error> {
        let url = format!(
            "{}/v2/bot/group/{}/member/{}",
            self.base_url, group_id, user_id
        );
        let resp = self
            .http_client
            .get(&url)
            .bearer_auth(&self.channel_access_token)
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let err_text = resp.text().await.unwrap_or_default();
            anyhow::bail!(
                "LINE get_group_member_profile failed [{}]: {}",
                status,
                err_text
            );
        }

        let profile: LineProfile = resp.json().await?;
        Ok(profile)
    }

    /// Display the loading animation in the 1-on-1 chat with the user.
    pub async fn start_loading_animation(
        &self,
        chat_id: &str,
        loading_seconds: Option<u32>,
    ) -> Result<(), anyhow::Error> {
        let url = format!("{}/v2/bot/chat/loading/start", self.base_url);
        let req = LoadingStartReq {
            chat_id: chat_id.to_string(),
            loading_seconds,
        };

        let resp = self
            .http_client
            .post(&url)
            .bearer_auth(&self.channel_access_token)
            .json(&req)
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let err_text = resp.text().await.unwrap_or_default();
            warn!(
                "LINE start_loading_animation failed [{}]: {}",
                status, err_text
            );
        } else {
            debug!("LINE loading animation started for {}", chat_id);
        }
        Ok(())
    }

    /// Reply to a message event using `replyToken`.
    /// Automatically chunks long text exceeding 5,000 characters into up to 5 messages.
    pub async fn reply_message(&self, reply_token: &str, text: &str) -> Result<(), anyhow::Error> {
        let messages = chunk_text_into_messages(text, 5000, 5);
        self.reply_messages(reply_token, messages).await
    }

    /// Reply to a message event with pre-structured messages.
    pub async fn reply_messages(
        &self,
        reply_token: &str,
        messages: Vec<LineMessage>,
    ) -> Result<(), anyhow::Error> {
        if messages.is_empty() {
            return Ok(());
        }
        let url = format!("{}/v2/bot/message/reply", self.base_url);
        let req = ReplyMessageReq {
            reply_token: reply_token.to_string(),
            messages,
            notification_disabled: None,
        };

        let resp = self
            .http_client
            .post(&url)
            .bearer_auth(&self.channel_access_token)
            .json(&req)
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let err_text = resp.text().await.unwrap_or_default();
            error!("LINE reply_message failed [{}]: {}", status, err_text);
            anyhow::bail!("LINE reply failed [{}]: {}", status, err_text);
        }

        info!("LINE reply sent successfully for token {}", reply_token);
        Ok(())
    }

    /// Push a message to a user or group.
    pub async fn push_message(&self, to: &str, text: &str) -> Result<(), anyhow::Error> {
        let messages = chunk_text_into_messages(text, 5000, 5);
        if messages.is_empty() {
            return Ok(());
        }
        let url = format!("{}/v2/bot/message/push", self.base_url);
        let req = PushMessageReq {
            to: to.to_string(),
            messages,
            notification_disabled: None,
        };

        let resp = self
            .http_client
            .post(&url)
            .bearer_auth(&self.channel_access_token)
            .json(&req)
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let err_text = resp.text().await.unwrap_or_default();
            error!("LINE push_message failed [{}]: {}", status, err_text);
            anyhow::bail!("LINE push failed [{}]: {}", status, err_text);
        }

        Ok(())
    }
}

/// Helper function to split text into chunks suitable for LINE message limits (5000 characters).
pub fn chunk_text_into_messages(text: &str, max_len: usize, max_chunks: usize) -> Vec<LineMessage> {
    if text.is_empty() {
        return vec![];
    }
    if text.chars().count() <= max_len {
        return vec![LineMessage::text(text)];
    }

    let mut chunks = Vec::new();
    let mut current = String::new();
    let mut current_len = 0;

    for line in text.split_inclusive('\n') {
        let line_chars = line.chars().count();
        if current_len + line_chars > max_len && !current.is_empty() {
            chunks.push(LineMessage::text(current));
            current = String::new();
            current_len = 0;
            if chunks.len() == max_chunks - 1 {
                break;
            }
        }

        if line_chars > max_len {
            // Line itself exceeds max_len, break by characters
            for c in line.chars() {
                if current_len >= max_len {
                    chunks.push(LineMessage::text(current));
                    current = String::new();
                    current_len = 0;
                    if chunks.len() == max_chunks - 1 {
                        break;
                    }
                }
                current.push(c);
                current_len += 1;
            }
        } else {
            current.push_str(line);
            current_len += line_chars;
        }
    }

    if !current.is_empty() && chunks.len() < max_chunks {
        chunks.push(LineMessage::text(current));
    }

    chunks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chunk_text_small() {
        let text = "Hello world";
        let chunks = chunk_text_into_messages(text, 100, 5);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].text, "Hello world");
    }

    #[test]
    fn test_chunk_text_by_lines() {
        let text = "line 1\nline 2\nline 3\nline 4\nline 5";
        let chunks = chunk_text_into_messages(text, 15, 5);
        assert!(chunks.len() >= 2);
        let joined: String = chunks.into_iter().map(|c| c.text).collect();
        assert_eq!(joined, text);
    }
}
