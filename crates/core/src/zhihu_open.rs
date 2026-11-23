use anyhow::{anyhow, ensure, Result};
use serde_json::Value;

pub async fn get(path: &str, secret: &str, query: &[(&str, &str)]) -> Result<Value> {
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(12))
        .build()?;
    let response = client
        .get(format!("https://developer.zhihu.com/api/v1/{path}"))
        .query(query)
        .bearer_auth(secret)
        .header(
            "X-Request-Timestamp",
            chrono::Utc::now().timestamp().to_string(),
        )
        .header("Content-Type", "application/json")
        .send()
        .await
        .map_err(|_| anyhow!("知乎开放平台连接失败"))?;
    ensure!(
        response.status().is_success(),
        "知乎开放平台返回 HTTP {}",
        response.status()
    );
    let body: Value = response
        .json()
        .await
        .map_err(|_| anyhow!("知乎开放平台响应格式异常"))?;
    let code = body["Code"]
        .as_i64()
        .ok_or_else(|| anyhow!("知乎开放平台响应缺少状态码"))?;
    ensure!(code == 0, "知乎开放平台接口返回错误码 {code}");
    body.get("Data")
        .filter(|data| data.is_object())
        .cloned()
        .ok_or_else(|| anyhow!("知乎开放平台没有返回数据"))
}
