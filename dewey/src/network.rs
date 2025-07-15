use std::env;

use dewey_macros::Serialize;

use crate::error;
use crate::serialization::Serialize;

pub const EMBED_DIM: usize = 1536;
pub const TOKEN_LIMIT: usize = 8192;

#[derive(Debug, Clone)]
struct RequestParams {
    host: String,
    path: String,
    model: String,
    authorization_token: String,
}

impl RequestParams {
    fn new() -> Self {
        Self {
            host: "api.openai.com".to_string(),
            path: "/v1/embeddings".to_string(),
            model: "text-embedding-3-small".to_string(),
            authorization_token: env::var("OPENAI_API_KEY")
                .expect("OPENAI_API_KEY environment variable not set"),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct NetworkEmbedding {
    pub id: u64,
    pub data: [f32; EMBED_DIM],
}

// Really, this should be async
// but idk yet how to adjust the #[tool] macro for async functions
// so here we are

fn embedding_api_call(
    params: &RequestParams,
    query: String,
) -> Result<Vec<NetworkEmbedding>, Box<dyn std::error::Error>> {
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()?;

    let body = serde_json::json!({
        "model": params.model,
        "input": vec![query]
    });

    let response = client
        .post(format!("https://{}{}", params.host, params.path))
        .header(
            "Authorization",
            format!("Bearer {}", params.authorization_token),
        )
        .json(&body)
        .send()?;

    let response_json: serde_json::Value = response.json()?;
    let data = response_json["data"].as_array().ok_or_else(|| {
        error!("Failed to parse data from JSON: {:?}", response_json);
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "Failed to parse data from JSON",
        )
    })?;

    let mut embeddings = Vec::new();
    for datum in data.iter() {
        let mut embedding = NetworkEmbedding {
            id: 0,
            data: [0.0; 1536],
        };

        for (i, value) in datum["embedding"].as_array().unwrap().iter().enumerate() {
            embedding.data[i] = value.as_f64().unwrap() as f32;
        }

        embeddings.push(embedding);
    }

    Ok(embeddings)
}

pub fn embed(query: String) -> Result<NetworkEmbedding, Box<dyn std::error::Error>> {
    match embedding_api_call(&RequestParams::new(), query.clone()) {
        Ok(embeddings) => Ok(embeddings[0].clone()),
        Err(e) => {
            error!("Failed to embed query \"{}\": {:?}", query, e);
            return Err(e);
        }
    }
}
