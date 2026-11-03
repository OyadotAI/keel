//! Local Oya operations use only this binding's owned tab and never replay uncertain effects.
use super::{Op, connection::Client};
use serde_json::json;
#[derive(Default)]
pub(super) struct Page {
    client: Option<Client>,
    tab: Option<u64>,
    pub url: Option<String>,
    analyzed: bool,
}
impl Page {
    pub fn reset(&mut self) {
        self.client = None;
        self.tab = None;
        self.url = None;
        self.analyzed = false;
    }
    pub fn connected(&self) -> bool {
        self.client.is_some()
    }
    pub async fn connect(&mut self, client: Client) -> Result<(), String> {
        let mut client = client;
        self.tab = None;
        self.analyzed = false;
        client.initialize().await?;
        self.client = Some(client);
        Ok(())
    }
    pub async fn run(&mut self, op: Op) -> Result<String, String> {
        let result = self.perform(op).await;
        if result.is_err() {
            self.analyzed = false;
        }
        result
    }
    async fn perform(&mut self, op: Op) -> Result<String, String> {
        let client=self.client.as_mut().ok_or("Local Oya is not connected. Use Retry connection in Browser settings; Keel did not switch browsers.")?;
        if let Op::Open { url } = op {
            self.analyzed = false;
            if let Some(tab) = self.tab {
                client.tool("switch_tab", json!({"tab_id":tab})).await?;
                let result = client.tool("navigate", json!({"url":url})).await?;
                self.url = Some(url);
                return Ok(result);
            }
            let result = client.tool("open_tab", json!({"url":url})).await?;
            let value: serde_json::Value = serde_json::from_str(&result)
                .map_err(|_| "Oya did not return an owned tab identity.")?;
            self.tab = Some(
                value["tab_id"]
                    .as_u64()
                    .ok_or("Oya did not return an owned tab identity.")?,
            );
            self.url = Some(url);
            return Ok(result);
        }
        let tab=self.tab.ok_or("Open a page with browser_open before using local Oya. Personal tabs are not available to this lane.")?;
        client.tool("switch_tab", json!({"tab_id":tab})).await?;
        match op {
            Op::Read => {
                let text = client
                    .tool("analyze_page", json!({"format":"markdown"}))
                    .await?;
                self.analyzed = true;
                Ok(text)
            }
            Op::Click { target } => {
                require_analysis(self.analyzed)?;
                self.analyzed = false;
                client.tool("click", json!({"element_id":target})).await
            }
            Op::Type {
                target,
                text,
                submit,
            } => {
                require_analysis(self.analyzed)?;
                self.analyzed = false;
                let typed = client
                    .tool("type", json!({"element_id":target,"text":text}))
                    .await?;
                if submit {
                    client.tool("press_key", json!({"key":"Enter"})).await
                } else {
                    Ok(typed)
                }
            }
            Op::Key { key } => {
                self.analyzed = false;
                client.tool("press_key", json!({"key":key})).await
            }
            Op::Scroll { direction } => {
                self.analyzed = false;
                client.tool("scroll", json!({"direction":direction})).await
            }
            Op::Back => {
                self.analyzed = false;
                client.tool("go_back", json!({})).await
            }
            Op::Open { .. } => unreachable!(),
        }
    }
    pub async fn show(&mut self) -> Result<String, String> {
        let tab = self.tab.ok_or("This lane has no owned Oya tab yet.")?;
        self.client
            .as_mut()
            .ok_or("Local Oya is disconnected.")?
            .tool("switch_tab", json!({"tab_id":tab}))
            .await
    }
}
fn require_analysis(analyzed: bool) -> Result<(), String> {
    if analyzed {
        Ok(())
    } else {
        Err(
            "Read the current owned page with browser_read before using an element reference."
                .into(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn element_actions_require_a_fresh_page_analysis() {
        assert!(require_analysis(false).is_err());
        assert!(require_analysis(true).is_ok());
    }
    #[tokio::test]
    async fn disconnected_local_browser_never_uses_an_embedded_or_personal_tab() {
        let mut page = Page::default();
        assert!(
            page.run(Op::Read)
                .await
                .unwrap_err()
                .contains("not connected")
        );
        assert!(page.show().await.unwrap_err().contains("no owned"));
    }
}
