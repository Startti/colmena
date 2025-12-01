// src/dag_engine/main.rs
use colmena::dag_engine::api;
use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(version, about = "Motor de ejecución de grafos DAG en Rust")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    Run { file_path: String },
    Serve {
        file_path: String,
        #[arg(long, default_value_t = 3000)]
        port: u16,
    },
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Run { file_path } => {
            println!("🚀 Modo Run: Cargando grafo desde {}", file_path);
            println!("Ejecutando grafo...");
            match api::run_dag(file_path).await {
                Ok(out) => println!("Output Final:\n{}", serde_json::to_string_pretty(&out)?),
                Err(e) => eprintln!("❌ Error: {}", e),
            }
        }
        Commands::Serve { file_path, port } => {
            println!("🌐 Modo Serve: Iniciando...");
            api::serve_dag(file_path, port).await?;
        }
    }

    Ok(())
}