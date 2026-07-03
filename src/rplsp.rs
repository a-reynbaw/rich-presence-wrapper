use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::*;
use tower_lsp::{Client, LanguageServer, LspService, Server};
use tokio::sync::Mutex;

#[derive(Debug)]
struct ActiveFile {
    uri: Url,
    filename: String,
    // TEMP
    #[allow(dead_code)]
    language_id: String,
}

#[derive(Debug)]
struct Backend {
    client: Client,
    current_file: Mutex<Option<ActiveFile>>,
}

impl Backend {
    async fn update_presence(&self, uri: &Url, lang_id: Option<String>) {
        let mut current = self.current_file.lock().await;
        let filename = uri.path().split('/').last().unwrap_or("Unknown");

        if current.as_ref().map(|d| &d.uri) != Some(uri) {
            *current = Some(ActiveFile {
                uri: uri.clone(),
                filename: filename.to_string(),
                language_id: lang_id.unwrap_or_else(|| "Plain Text".to_string()),
            });
        }

        self.client.log_message(MessageType::INFO, format!("updating {}", filename)).await;
    }

    async fn clear_presence(&self, uri: &Url) {
        let mut current = self.current_file.lock().await;

        if let Some(active) = current.as_ref() {
            if &active.uri == uri {
                self.client.log_message(MessageType::INFO, format!("cleared file {}", &active.filename)).await;
                *current = None;
            }
        }
    }
        
}

#[tower_lsp::async_trait]
impl LanguageServer for Backend {
    async fn initialize(&self, _: InitializeParams) -> Result<InitializeResult> {
        Ok(InitializeResult {
            capabilities: ServerCapabilities {
                hover_provider: Some(HoverProviderCapability::Simple(true)),
                text_document_sync: Some(TextDocumentSyncCapability::Kind(TextDocumentSyncKind::FULL)),

                ..Default::default()
            },
            ..Default::default()
        })
    }

    async fn initialized(&self, _: InitializedParams) {
        self.client
        .log_message(MessageType::INFO, "server initialized")
        .await;
    }

    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        self.update_presence(
            &params.text_document.uri,
            Some(params.text_document.language_id)
        ).await;

        self.client
            .log_message(MessageType::INFO, format!("opened file {}", params.text_document.uri))
            .await;
    }
    
    async fn hover(&self, params: HoverParams) -> Result<Option<Hover>> {
        self.update_presence(
            &params.text_document_position_params.text_document.uri,
            None,
        ).await;

        Ok(Some(Hover {
            contents: HoverContents::Scalar(
                MarkedString::String("hovering file".to_string())
            ),
            range: None
        }))
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        self.update_presence(
            &params.text_document.uri,
            None,
        ).await;

        self.client
            .log_message(MessageType::INFO, format!("changed file {}", params.text_document.uri))
            .await;
    }

    async fn did_save(&self, params: DidSaveTextDocumentParams) {
        self.update_presence(
            &params.text_document.uri,
            None,
        ).await;

        self.client
            .log_message(MessageType::INFO, format!("saved file {}", params.text_document.uri))
            .await;
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        self.clear_presence(&params.text_document.uri).await;
            
        self.client
            .log_message(MessageType::INFO, format!("cosed file {}", params.text_document.uri))
            .await;
    }
}

#[tokio::main]
async fn main() {
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();

    let (service, socket) = LspService::new(|client| Backend {
        client,
        current_file: Mutex::new(None),
    });
    Server::new(stdin, stdout, socket).serve(service).await;
}