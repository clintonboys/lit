use anyhow::Result;
use colored::Colorize;

use crate::core::config::LitConfig;
use crate::core::generation_record::{GenerationRecord, format_cost, format_tokens};
use crate::core::style;

pub async fn run(last: bool, breakdown: bool) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let (_config, root) = LitConfig::find_and_load(&cwd)?;

    let generations_dir = root.join(".lit").join("generations");
    let records = GenerationRecord::list(&generations_dir)?;

    if records.is_empty() {
        eprintln!("{}", "No generation records found.".dimmed());
        eprintln!("{}", style::hint("Hint: Run `lit regenerate` first."));
        return Ok(());
    }

    if last {
        let latest = &records[0];
        print_record_summary(latest, breakdown);
    } else {
        print_aggregate(&records, breakdown);
    }

    Ok(())
}

fn print_record_summary(record: &GenerationRecord, breakdown: bool) {
    eprintln!("{}", style::header("Last Generation"));
    eprintln!(
        "  {:<16} {}",
        "Time:".dimmed(),
        record.timestamp.format("%Y-%m-%d %H:%M:%S UTC").to_string().dimmed()
    );
    eprintln!("  {:<16} {}", "Model:".dimmed(), record.model);
    eprintln!(
        "  {:<16} {} total ({} generated, {} cached, {} skipped)",
        "Prompts:".dimmed(),
        record.summary.total_prompts.to_string().bold(),
        record.summary.cache_misses.to_string().yellow(),
        record.summary.cache_hits.to_string().green(),
        record.summary.skipped.to_string().dimmed()
    );
    eprintln!(
        "  {:<16} {} in / {} out",
        "Tokens:".dimmed(),
        format_tokens(record.summary.total_tokens_in).dimmed(),
        format_tokens(record.summary.total_tokens_out).dimmed()
    );
    eprintln!(
        "  {:<16} {}",
        "Cost:".dimmed(),
        style::cost(&format_cost(record.summary.total_cost_usd))
    );
    eprintln!(
        "  {:<16} {}",
        "Duration:".dimmed(),
        format!("{:.1}s", record.summary.total_duration_ms as f64 / 1000.0).dimmed()
    );

    if breakdown {
        eprintln!();
        eprintln!("  {}", "Per-prompt breakdown:".bold());
        let mut prompts: Vec<_> = record.prompts.iter().collect();
        prompts.sort_by(|a, b| b.cost_usd.partial_cmp(&a.cost_usd).unwrap());

        for p in &prompts {
            let status = if p.from_cache {
                "cached".green()
            } else {
                "generated".yellow()
            };
            eprintln!(
                "    {} ({}) — {} in / {} out — {}",
                p.prompt_path.display(),
                status,
                format_tokens(p.tokens_in).dimmed(),
                format_tokens(p.tokens_out).dimmed(),
                style::cost(&format_cost(p.cost_usd)),
            );
        }
    }
}

fn print_aggregate(records: &[GenerationRecord], breakdown: bool) {
    let total_cost: f64 = records.iter().map(|r| r.summary.total_cost_usd).sum();
    let total_tokens_in: u64 = records.iter().map(|r| r.summary.total_tokens_in).sum();
    let total_tokens_out: u64 = records.iter().map(|r| r.summary.total_tokens_out).sum();
    let total_cache_hits: usize = records.iter().map(|r| r.summary.cache_hits).sum();
    let total_cache_misses: usize = records.iter().map(|r| r.summary.cache_misses).sum();

    eprintln!(
        "{}",
        style::header(&format!("Cost Summary ({} generation(s))", records.len()))
    );
    eprintln!(
        "  {:<16} {}",
        "Total cost:".dimmed(),
        style::cost(&format_cost(total_cost))
    );
    eprintln!(
        "  {:<16} {} in / {} out",
        "Total tokens:".dimmed(),
        format_tokens(total_tokens_in).dimmed(),
        format_tokens(total_tokens_out).dimmed()
    );
    eprintln!(
        "  {:<16} {} hit(s), {} miss(es)",
        "Cache:".dimmed(),
        total_cache_hits.to_string().green(),
        total_cache_misses.to_string().yellow()
    );

    if let Some(first) = records.last() {
        eprintln!(
            "  {:<16} {}",
            "First run:".dimmed(),
            first.timestamp.format("%Y-%m-%d %H:%M:%S UTC").to_string().dimmed()
        );
    }
    if let Some(latest) = records.first() {
        eprintln!(
            "  {:<16} {}",
            "Latest run:".dimmed(),
            latest.timestamp.format("%Y-%m-%d %H:%M:%S UTC").to_string().dimmed()
        );
    }

    if breakdown {
        eprintln!();
        eprintln!("  {}", "Per-generation breakdown:".bold());
        for (i, record) in records.iter().enumerate() {
            eprintln!(
                "    {}. {} — {} ({} generated, {} cached) — {} in / {} out — {}",
                (i + 1).to_string().dimmed(),
                record.timestamp.format("%Y-%m-%d %H:%M:%S").to_string().dimmed(),
                record.summary.total_prompts,
                record.summary.cache_misses.to_string().yellow(),
                record.summary.cache_hits.to_string().green(),
                format_tokens(record.summary.total_tokens_in).dimmed(),
                format_tokens(record.summary.total_tokens_out).dimmed(),
                style::cost(&format_cost(record.summary.total_cost_usd)),
            );
        }
    }
}
