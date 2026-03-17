use reqwest::{Client, multipart};
use serde::{Deserialize, Serialize};
use serenity::builder::EditChannel;
use serenity::http::Http;
use serenity::model::id::ChannelId;
use std::collections::HashMap;
use std::env;
use std::fs;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::time::sleep;
use image::{Rgba, ImageFormat, open};
use imageproc::drawing::draw_text_mut;
use rusttype::{Font, Scale};
use std::io::Cursor;

#[derive(Debug, Deserialize)]
struct CoinPaprikaTicker {
    quotes: CoinPaprikaQuotes,
}

#[derive(Debug, Deserialize)]
struct CoinPaprikaQuotes {
    #[serde(rename = "USD")]
    usd: CoinPaprikaQuote,
}

#[derive(Debug, Deserialize)]
struct CoinPaprikaQuote {
    price: f64,
    percent_change_1h: Option<f64>,
    percent_change_12h: Option<f64>,
    percent_change_24h: Option<f64>,
    percent_change_7d: Option<f64>,
}

#[derive(Debug, Deserialize)]
struct MetalApiResponse {
    price: f64,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct PriceEntry {
    timestamp: u64,
    price: f64,
}

#[derive(Debug, Serialize, Deserialize, Default)]
struct PriceHistory {
    history: HashMap<String, Vec<PriceEntry>>,
}

struct PriceData {
    price: f64,
    p1h: Option<f64>,
    p12h: Option<f64>,
    p24h: Option<f64>,
    p7d: Option<f64>,
}

async fn get_price_with_retry(client: &Client, coin_id: &str) -> Result<PriceData, Box<dyn std::error::Error>> {
    let mut last_err = None;
    for _ in 0..3 {
        let url = format!("https://api.coinpaprika.com/v1/tickers/{}", coin_id);
        match client.get(&url).timeout(Duration::from_secs(15)).send().await {
            Ok(resp) => {
                let ticker: CoinPaprikaTicker = resp.json().await?;
                return Ok(PriceData {
                    price: ticker.quotes.usd.price,
                    p1h: ticker.quotes.usd.percent_change_1h,
                    p12h: ticker.quotes.usd.percent_change_12h,
                    p24h: ticker.quotes.usd.percent_change_24h,
                    p7d: ticker.quotes.usd.percent_change_7d,
                });
            }
            Err(e) => {
                last_err = Some(e);
                sleep(Duration::from_secs(2)).await;
            }
        }
    }
    Err(Box::new(last_err.unwrap()))
}

async fn get_metal_with_retry(client: &Client, symbol: &str) -> Result<f64, Box<dyn std::error::Error>> {
    let mut last_err = None;
    for _ in 0..3 {
        let url = format!("https://api.gold-api.com/price/{}", symbol);
        match client.get(&url).timeout(Duration::from_secs(15)).send().await {
            Ok(resp) => {
                let data: MetalApiResponse = resp.json().await?;
                return Ok(data.price);
            }
            Err(e) => {
                last_err = Some(e);
                sleep(Duration::from_secs(2)).await;
            }
        }
    }
    Err(Box::new(last_err.unwrap()))
}

fn generate_card(ticker: &str, data: &PriceData) -> Vec<u8> {
    let mut img = open("background.png").expect("Failed to load background.png").to_rgba8();
    let font_data = include_bytes!("/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf");
    let font = Font::try_from_bytes(font_data as &[u8]).expect("Error constructing Font");

    let text_white = Rgba([255, 255, 255, 255]);
    let text_green = Rgba([0, 255, 127, 255]);
    let text_red = Rgba([255, 45, 85, 255]);

    let left_padding = 120;
    let top_padding = 130;

    draw_text_mut(&mut img, text_white, left_padding, top_padding, Scale::uniform(85.0), &font, &format!("{} / USD", ticker));
    draw_text_mut(&mut img, text_white, left_padding, top_padding + 110, Scale::uniform(220.0), &font, &format!("${:.2}", data.price));

    let stats_start_y = top_padding + 380;
    let stats = vec![
        ("1H  Change:", data.p1h, stats_start_y),
        ("12H Change:", data.p12h, stats_start_y + 80),
        ("24H Change:", data.p24h, stats_start_y + 160),
        ("7D  Change:", data.p7d, stats_start_y + 240),
    ];

    for (label, val_opt, y_pos) in stats {
        draw_text_mut(&mut img, text_white, left_padding, y_pos, Scale::uniform(65.0), &font, label);
        let (color, text) = match val_opt {
            Some(v) if v > 0.0 => (text_green, format!("↗️ {:.2}%", v)),
            Some(v) if v < 0.0 => (text_red, format!("↘️ {:.2}%", v.abs())),
            _ => (Rgba([140, 140, 140, 255]), "──".to_string()),
        };
        draw_text_mut(&mut img, color, left_padding + 480, y_pos, Scale::uniform(65.0), &font, &text);
    }

    let mut buffer = Cursor::new(Vec::new());
    img.write_to(&mut buffer, ImageFormat::Png).unwrap();
    buffer.into_inner()
}

async fn send_image_webhook(webhook_url: &str, image_bytes: Vec<u8>) {
    let client = Client::new();
    let part = multipart::Part::bytes(image_bytes).file_name("update.png").mime_str("image/png").unwrap();
    let form = multipart::Form::new().part("file", part);
    let _ = client.post(webhook_url).multipart(form).send().await;
}

async fn update_channel_name(http: &Http, channel_id: u64, new_name: &str) {
    let channel_id = ChannelId::new(channel_id);
    let edit_channel = EditChannel::new().name(new_name);
    let _ = channel_id.edit(http, edit_channel).await;
}

fn format_price_fullwidth(price: f64) -> String {
    let formatted_price = format!("{:.2}", price);
    formatted_price.chars().map(|c| match c {
        '0' => '０', '1' => '１', '2' => '２', '3' => '３', '4' => '４',
        '5' => '５', '6' => '６', '7' => '７', '8' => '８', '9' => '９',
        '.' => '．', _ => c,
    }).collect::<String>()
}

fn calculate_change(current: f64, history: &[PriceEntry], seconds_ago: u64) -> Option<f64> {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs();
    let target_time = now.saturating_sub(seconds_ago);
    let closest = history.iter().filter(|e| e.timestamp <= target_time).max_by_key(|e| e.timestamp)?;
    if now.saturating_sub(closest.timestamp) > (seconds_ago + 3600) { return None; }
    Some(((current - closest.price) / closest.price) * 100.0)
}

#[tokio::main]
async fn main() {
    dotenv::dotenv().ok();
    let discord_token = env::var("DISCORD_BOT_TOKEN").unwrap();
    let http = Http::new(&discord_token);
    let client = Client::new();
    let history_path = "price_history.json";
    let assets_config = vec![
        ("BTC", "btc-bitcoin", "BTC_CHANNEL_ID", "BTC_WEBHOOK", "📈╭⋅", true),
        ("TON", "ton-toncoin", "TON_CHANNEL_ID", "TON_WEBHOOK", "💎│⋅", true),
        ("SOL", "sol-solana", "SOL_CHANNEL_ID", "SOL_WEBHOOK", "🌞│⋅", true),
        ("BNB", "bnb-binance-coin", "BNB_CHANNEL_ID", "BNB_WEBHOOK", "🟡│⋅", true),
        ("ETH", "eth-ethereum", "ETH_CHANNEL_ID", "ETH_WEBHOOK", "🟣│⋅", true),
        ("XMR", "xmr-monero", "XMR_CHANNEL_ID", "XMR_WEBHOOK", "🔐│⋅", true),
        ("GOLD", "XAU", "GOLD_CHANNEL_ID", "GOLD_WEBHOOK", "🥇│⋅", false),
        ("SILVER", "XAG", "SILVER_CHANNEL_ID", "SILVER_WEBHOOK", "🥈╰⋅", false),
    ];
    loop {
        let mut history_data: PriceHistory = fs::read_to_string(history_path)
            .ok().and_then(|content| serde_json::from_str(&content).ok()).unwrap_or_default();
        let current_time = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
        let thirty_days_ago = current_time - (30 * 24 * 60 * 60);
        for (ticker, api_id, channel_env, webhook_env, emoji, is_crypto) in &assets_config {
            let channel_id: u64 = env::var(channel_env).unwrap().parse().unwrap();
            let webhook_url = env::var(webhook_env).unwrap();
            let data_res = if *is_crypto {
                get_price_with_retry(&client, api_id).await
            } else {
                get_metal_with_retry(&client, api_id).await.map(|p| PriceData {
                    price: p, p1h: None, p12h: None, p24h: None, p7d: None,
                })
            };
            if let Ok(mut data) = data_res {
                if !is_crypto {
                    let vec = history_data.history.entry(ticker.to_string()).or_insert(Vec::new());
                    vec.push(PriceEntry { timestamp: current_time, price: data.price });
                    vec.retain(|e| e.timestamp > thirty_days_ago);
                    data.p1h = calculate_change(data.price, vec, 3600);
                    data.p12h = calculate_change(data.price, vec, 43200);
                    data.p24h = calculate_change(data.price, vec, 86400);
                    data.p7d = calculate_change(data.price, vec, 604800);
                }
                let image_bytes = generate_card(ticker, &data);
                send_image_webhook(&webhook_url, image_bytes).await;
                let fullwidth = format_price_fullwidth(data.price);
                let name = format!("{}{}{}", emoji, ticker.to_lowercase(), fullwidth);
                update_channel_name(&http, channel_id, &name).await;
                sleep(Duration::from_secs(2)).await;
            }
        }
        let _ = fs::write(history_path, serde_json::to_string(&history_data).unwrap());
        sleep(Duration::from_secs(300)).await;
    }
}
