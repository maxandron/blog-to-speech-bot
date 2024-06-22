use bytes::Bytes;
use dotenv::dotenv;
use std::future::Future;
use std::pin::Pin;
use std::string::String;
use std::sync::Arc;
use std::{error::Error, process::Stdio};
use teloxide::RequestError;
use teloxide::{prelude::*, types::InputFile};
use thirtyfour::WebDriver;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::Mutex;

trait HandleError<T> {
    fn handle_error(
        self,
        bot: Bot,
        chat_id: teloxide::types::ChatId,
        error_message: String,
    ) -> Pin<Box<dyn Future<Output = Result<T, RequestError>> + Send>>;
}

/// I wanted a less verbose way to handle the errors in the repl closure.
/// By calling `handle_error` on any `Result` type and passing it the bot, the chat_id and an error message,
/// it will send a message to the chat with the error message and the error that occurred.
/// Additionally for convenience, it returns a `RequestError` so that the error can be propagated up the chain using `?`.
impl<T, E> HandleError<T> for Result<T, E>
where
    T: Send + 'static,
    E: std::fmt::Debug + Send + 'static,
{
    fn handle_error(
        self,
        bot: Bot,
        chat_id: teloxide::types::ChatId,
        error_message: String,
    ) -> Pin<Box<dyn Future<Output = Result<T, RequestError>> + Send>> {
        Box::pin(async move {
            self.map_err(|e| {
                let chat_id_clone = chat_id.clone();
                tokio::spawn(async move {
                    bot.send_message(chat_id_clone, format!("Error: {error_message} {e:?}"))
                        .await
                        .unwrap();
                });
                RequestError::Io(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    format!("Error"),
                ))
            })
        })
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error + Send + Sync>> {
    println!("Starting...");

    // Load the .env file.
    dotenv().ok();

    // Kill any existing geckodriver processes
    println!("Killing existing geckodriver processes if any are running");
    let _ = tokio::process::Command::new("pkill")
        .arg("geckodriver")
        .output()
        .await;

    let geckodriver_path = std::env::var("GECKODRIVER_PATH").unwrap_or("geckodriver".to_string());
    println!("Running geckodriver ({geckodriver_path})");
    let child = tokio::process::Command::new(geckodriver_path)
        .stdout(Stdio::piped())
        .spawn()
        .expect("Failed to start geckodriver");

    println!("Waiting for geckodriver to start...");
    let stdout = child.stdout.expect("Failed to get stdout");

    // Combine stdout and stderr into a single stream.
    let mut reader = BufReader::new(stdout).lines();

    while let Some(line) = reader.next_line().await? {
        println!("Received: {line}");
        if line.contains("Listening") {
            break;
        }
    }
    println!("Geckodriver started");

    println!("Initializing driver...");
    let driver = Arc::new(Mutex::new(init_driver().await?));

    // Register one command that responds filters any textual message
    // Pass the web driver to the command.
    let bot = Bot::from_env();
    println!("Starting bot...");
    teloxide::repl(bot, move |bot: Bot, msg: Message| {
        let driver = driver.clone();
        async move {
            let url = match msg.text() {
                Some(text) => text,
                None => return Ok(()),
            };
            println!("Received URL: {}", url);
            bot.send_message(msg.chat.id, "Got it! Working on it. It may take a while...")
                .await?;
            // Navigate to the page.
            let blog_text = {
                let driver = driver.lock().await;
                get_blog_text(&driver, &url).await
            }
            .handle_error(
                bot.clone(),
                msg.chat.id,
                "Error retrieving blog text".to_string(),
            )
            .await?;

            let len = blog_text.len();
            println!("Retrieved blog text of length {len}");

            println!("Editing text...");
            let edited_blog_text = edit_text(&blog_text)
                .await
                .handle_error(bot.clone(), msg.chat.id, "Error editing text".to_string())
                .await?;

            // Loop on the text and break it into chunks of at most 4096 characters.
            // But break on word boundaries.
            for (i, chunk) in chunk_text_by_lines(&edited_blog_text, 4096)
                .iter()
                .enumerate()
            {
                println!("Converting part {i} to speech...");
                let audio_bytes = text_to_speech(&chunk)
                    .await
                    .handle_error(
                        bot.clone(),
                        msg.chat.id,
                        "Error converting text to speech".to_string(),
                    )
                    .await?;

                println!("Sending audio of part {i}...");
                bot.send_audio(
                    msg.chat.id,
                    InputFile::memory(audio_bytes).file_name(format!("part_{i}.mp3")),
                )
                .await?;
            }

            Ok(())
        }
    })
    .await;

    Ok(())
}

fn chunk_text_by_lines(text: &str, max_chunk_size: usize) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut current_chunk = String::new();

    for line in text.lines() {
        // Check if adding this line would exceed the max chunk size
        if current_chunk.len() + line.len() + 1 > max_chunk_size {
            // +1 for the newline character that will be added
            chunks.push(current_chunk);
            current_chunk = String::new();
        }

        if !current_chunk.is_empty() {
            // Add a newline before adding the line if it's not the first line in the chunk
            current_chunk.push('\n');
        }
        current_chunk.push_str(line);
    }

    // Don't forget to add the last chunk if it's not empty
    if !current_chunk.is_empty() {
        chunks.push(current_chunk);
    }

    chunks
}

async fn init_driver() -> Result<WebDriver, Box<dyn Error + Send + Sync>> {
    let mut caps = thirtyfour::DesiredCapabilities::firefox();
    caps.set_headless()?;
    caps.add_arg("--no-sandbox")?;
    caps.add_arg("--disable-dev-shm-usage")?;
    let driver = thirtyfour::WebDriver::new("http://127.0.0.1:4444", caps).await?;

    Ok(driver)
}

async fn get_blog_text(
    driver: &thirtyfour::WebDriver,
    blog: &str,
) -> Result<String, Box<dyn Error + Send + Sync>> {
    driver.goto(blog).await?;

    let article = driver.find(thirtyfour::By::Tag("article")).await?;

    let paragraphs = article.find_all(thirtyfour::By::Tag("p")).await?;

    let mut text = String::new();

    for p in paragraphs {
        text.push_str(&p.text().await?);
        text.push_str("\n");
    }

    Ok(text)
}

async fn edit_text(text: &str) -> Result<String, Box<dyn Error + Send + Sync + 'static>> {
    let bearer_token = std::env::var("OPENAI_BEARER_TOKEN")?;
    let client = reqwest::Client::new();
    let response = client
        .post("https://api.openai.com/v1/chat/completions")
        .header("Authorization", format!("Bearer {}", bearer_token))
        .header("Content-Type", "application/json")
        .json(&serde_json::json!({
            "model": "gpt-4o",
            "messages": [
                {
                    "role": "user",
                    "content": [
                        {
                            "type": "text",
                            "text": "Given text from a blog post:\n- Remove any introductory statement or metadata\n- Redact code blocks and replace them with a short technical explanation of their content. Start with \"EDIT:\". End with \"END OF EDIT.\".\nEmojis or other characters that cannot be pronounced should be removed.\nYour response will be directly read of the user - so avoid any additional content besides the edited post\n\nOK?"
                        }
                    ]
                },
                {
                    "role": "assistant",
                    "content": [
                        {
                            "type": "text",
                            "text": "Okay, just provide the text from the blog post and I'\''ll make the necessary edits."
                        }
                    ]
                },
                {
                    "role": "user",
                    "content": [
                        {
                            "type": "text",
                            "text": text
                        }
                    ]
                }
            ],
            "temperature": 1,
            "top_p": 1,
            "frequency_penalty": 0,
            "presence_penalty": 0
        })).send().await?;
    let status = response.status();
    if !status.is_success() {
        let text = response.text().await?;
        return Err(Box::new(std::io::Error::new(
            std::io::ErrorKind::Other,
            format!("Failed to edit text: {status}: {text}"),
        )));
    }

    let text = response.json::<serde_json::Value>().await?;
    let text = text["choices"][0]["message"]["content"].as_str().unwrap();
    Ok(text.to_string())
}

async fn text_to_speech(text: &str) -> Result<Bytes, Box<dyn Error + Send + Sync>> {
    let bearer_token = std::env::var("OPENAI_BEARER_TOKEN")?;
    let client = reqwest::Client::new();
    let response = client
        .post("https://api.openai.com/v1/audio/speech")
        .header("Authorization", format!("Bearer {}", bearer_token))
        .header("Content-Type", "application/json")
        .json(&serde_json::json!({
            "model": "tts-1",
            "input": text,
            "voice": "nova"
        }))
        .send()
        .await?;
    let status = response.status();
    if !status.is_success() {
        let text = response.text().await?;
        return Err(Box::new(std::io::Error::new(
            std::io::ErrorKind::Other,
            format!("Failed to convert text to speech: {status}: {text}"),
        )));
    }

    let audio_bytes_result = response.bytes().await;
    let audio_bytes = match audio_bytes_result {
        Ok(audio_bytes) => audio_bytes,
        Err(e) => {
            return Err(Box::new(std::io::Error::new(
                std::io::ErrorKind::Other,
                format!("Failed to read audio bytes: {e}"),
            )));
        }
    };

    Ok(audio_bytes)
}
