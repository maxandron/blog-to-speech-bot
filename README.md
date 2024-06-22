# Blog blog to speech converter telegram bot

A simple project I made to learn Rust.

This is a telegram bot that converts a blog post to speech by:

- Fetching the text of the blog post using Selenium (geckodriver and Firefox). Using the crate thirtyfour.
- Editing the text by sending it to GPT-4o to redact code blocks, metadata, and unreadable text.
- Converting the text to speech using OpenAI's TTS API.

## Setup

1. Download geckodriver
2. Copy the .env.example file to .env and fill in the required fields
3. cargo run
