// Declara los módulos principales (las carpetas en src/dag_engine/)
// para que `main.rs` pueda "verlos".
pub mod application;
pub mod domain;
pub mod infrastructure;

// --- Importaciones ---
use crate::application::run_use_case::DagRunUseCase;
use crate::domain::graph::Graph;
use crate::infrastructure::registry::HashMapNodeRegistry;
use clap::Parser;
use std::sync::Arc;

/// Define la interfaz de línea de comandos (CLI)
#[derive(Parser, Debug)]
#[command(version, about = "Motor de ejecución de grafos DAG en Rust")]
struct Cli {
    /// La ruta al archivo graph.json que se va a ejecutar
    #[arg(index = 1)]
    file_path: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 1. Parsear los argumentos de la línea de comandos
    let cli = Cli::parse();

    // --- Inyección de Dependencias (El "Ensamblaje") ---
    //
    // 2. Crear el Adaptador de Infraestructura
    let registry = Arc::new(HashMapNodeRegistry::new());

    // 3. Crear el Caso de Uso de Aplicación e inyectar
    //    el adaptador (el "Puerto")
    let run_use_case = DagRunUseCase::new(registry);
    // --- Fin de Inyección ---

    // 4. Cargar el grafo (este es el "Adaptador" de entrada,
    //    desde el sistema de archivos)
    println!("Cargando grafo desde: {}", &cli.file_path);
    let file_content = tokio::fs::read_to_string(&cli.file_path).await?;
    let graph: Graph = serde_json::from_str(&file_content)?;

    // 5. Ejecutar el Caso de Uso
    println!("Ejecutando grafo...");
    match run_use_case.execute(graph).await {
        Ok(final_output) => {
            println!("--- Ejecución Finalizada ---");
            println!(
                "Output Final:\n{}",
                serde_json::to_string_pretty(&final_output)?
            );
        }
        Err(e) => {
            eprintln!("Error en la ejecución: {}", e);
        }
    }

    Ok(())
}