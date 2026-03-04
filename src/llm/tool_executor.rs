use reqwest::Client;

use crate::error::AppError;
use crate::gateway::acr_client::AgentConfig;
use crate::gateway::tenant_client::DataSource;

use super::client::ToolCall;

/// Result of executing a single tool call against an external data source.
#[derive(Debug)]
pub struct ToolResult {
    /// The tool_call_id this result corresponds to.
    pub tool_call_id: String,
    /// The tool function name (for logging/debugging).
    pub function_name: String,
    /// The JSON response from the external API (or error message).
    pub output: String,
}

/// Execute tool calls against tenant data sources.
///
/// For each tool call:
/// 1. Look up the tool name in `agent_config.tool_permissions`
/// 2. Find the matching data source + route config
/// 3. Construct the HTTP request (method, path, params from LLM arguments)
/// 4. Call the external API
/// 5. Return the response as a string for the LLM
///
/// If a tool is not found or the external API fails, a descriptive error
/// message is returned to the LLM (not propagated as an AppError) so the
/// LLM can gracefully handle the failure.
#[derive(Clone)]
pub struct ToolExecutor {
    client: Client,
}

impl ToolExecutor {
    pub fn new(client: Client) -> Self {
        Self { client }
    }

    /// Execute a batch of tool calls in parallel.
    pub async fn execute_batch(
        &self,
        tool_calls: &[ToolCall],
        agent_config: &AgentConfig,
        data_sources: &[DataSource],
    ) -> Vec<ToolResult> {
        let futures: Vec<_> = tool_calls
            .iter()
            .map(|tc| self.execute_one(tc, agent_config, data_sources))
            .collect();

        futures::future::join_all(futures).await
    }

    /// Execute a single tool call.
    async fn execute_one(
        &self,
        tool_call: &ToolCall,
        agent_config: &AgentConfig,
        data_sources: &[DataSource],
    ) -> ToolResult {
        let function_name = &tool_call.function.name;

        // 1. Check if tool is permitted in agent config
        let permitted = agent_config
            .tool_permissions
            .iter()
            .any(|tp| tp.tool_name == *function_name);

        if !permitted {
            return ToolResult {
                tool_call_id: tool_call.id.clone(),
                function_name: function_name.clone(),
                output: format!("Error: tool '{function_name}' is not permitted for this agent"),
            };
        }

        // 2. Find the data source + route config for this tool
        let route = self.find_route(function_name, data_sources);
        let Some((data_source, route_config)) = route else {
            return ToolResult {
                tool_call_id: tool_call.id.clone(),
                function_name: function_name.clone(),
                output: format!("Error: no route config found for tool '{function_name}'"),
            };
        };

        // 3. Parse LLM arguments
        let args: serde_json::Value =
            serde_json::from_str(&tool_call.function.arguments).unwrap_or_default();

        // 4. Execute the HTTP request
        match self
            .call_data_source(data_source, route_config, &args)
            .await
        {
            Ok(response_body) => ToolResult {
                tool_call_id: tool_call.id.clone(),
                function_name: function_name.clone(),
                output: response_body,
            },
            Err(e) => ToolResult {
                tool_call_id: tool_call.id.clone(),
                function_name: function_name.clone(),
                output: format!("Error calling external API: {e}"),
            },
        }
    }

    /// Find the data source and route config for a tool name.
    ///
    /// Scans all active data sources' `route_configs` for a key matching the tool name.
    /// Returns the first match.
    fn find_route<'a>(
        &self,
        tool_name: &str,
        data_sources: &'a [DataSource],
    ) -> Option<(&'a DataSource, &'a serde_json::Value)> {
        for ds in data_sources {
            if !ds.is_active {
                continue;
            }
            if let Some(route) = ds.route_configs.get(tool_name) {
                return Some((ds, route));
            }
        }
        None
    }

    /// Call an external data source API.
    ///
    /// Route config format (from ACR):
    /// ```json
    /// {
    ///   "method": "GET",
    ///   "path": "/doctors",
    ///   "query_params": ["specialty", "location"]
    /// }
    /// ```
    /// or
    /// ```json
    /// {
    ///   "method": "POST",
    ///   "path": "/appointments",
    ///   "body_template": true
    /// }
    /// ```
    async fn call_data_source(
        &self,
        data_source: &DataSource,
        route_config: &serde_json::Value,
        args: &serde_json::Value,
    ) -> Result<String, AppError> {
        let method = route_config
            .get("method")
            .and_then(|v| v.as_str())
            .unwrap_or("GET")
            .to_uppercase();

        let path = route_config
            .get("path")
            .and_then(|v| v.as_str())
            .unwrap_or("/");

        let url = format!(
            "{}{}",
            data_source.base_url.trim_end_matches('/'),
            path
        );

        let mut request = match method.as_str() {
            "POST" => self.client.post(&url),
            "PUT" => self.client.put(&url),
            "PATCH" => self.client.patch(&url),
            "DELETE" => self.client.delete(&url),
            _ => self.client.get(&url),
        };

        // Add auth if data source has a credential
        if let Some(ref _cred_ref) = data_source.credential_ref {
            // TODO: Resolve credential from vault/secrets manager
            // For MVP, the mock data sources don't require auth
        }

        // For GET requests: map LLM args to query parameters
        if method == "GET" {
            if let Some(params) = route_config.get("query_params").and_then(|v| v.as_array()) {
                let query_pairs: Vec<(String, String)> = params
                    .iter()
                    .filter_map(|p| {
                        let key = p.as_str()?;
                        let val = args.get(key)?;
                        let val_str = match val {
                            serde_json::Value::String(s) => s.clone(),
                            other => other.to_string(),
                        };
                        Some((key.to_string(), val_str))
                    })
                    .collect();

                request = request.query(&query_pairs);
            }
        } else {
            // For POST/PUT/PATCH: send LLM args as JSON body
            request = request.json(args);
        }

        let resp = request
            .send()
            .await
            .map_err(|e| AppError::Downstream(format!("tool API call failed: {e}")))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(AppError::Downstream(format!(
                "tool API returned {status}: {body}"
            )));
        }

        resp.text()
            .await
            .map_err(|e| AppError::Downstream(format!("failed to read tool response: {e}")))
    }
}
