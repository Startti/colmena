# Documento de Diseño y Desarrollo (DDS) - Sistema RAG Avanzado v2.2

## Tabla de Contenido
1.  [Resumen Ejecutivo](#1-resumen-ejecutivo)
2.  [Principios Arquitectónicos](#2-principios-arquitectónicos)
3.  [Diagrama de Arquitectura](#3-diagrama-de-arquitectura)
4.  [Diseño Detallado del Dominio](#4-diseño-detallado-del-dominio)
5.  [Diseño Detallado de la Aplicación](#5-diseño-detallado-de-la-aplicación)
6.  [Diseño Detallado de la Infraestructura](#6-diseño-detallado-de-la-infraestructura)
7.  [Gestión de Configuración](#7-gestión-de-configuración)
8.  [Estructura de Directorios](#8-estructura-de-directorios)
9.  [Análisis Competitivo y Ventajas](#9-análisis-competitivo-y-ventajas)
10. [Plan de Implementación por Fases](#10-plan-de-implementación-por-fases)
11. [Dependencias Requeridas (`Cargo.toml`)](#11-dependencias-requeridas-cargotoml)

---

## 1. Resumen Ejecutivo
Este documento detalla el diseño de un sistema de **Retrieval-Augmented Generation (RAG)** de alto rendimiento, modular y extensible, construido íntegramente en **Rust**. El sistema se fundamenta en la **Arquitectura Hexagonal** para un desacoplamiento total de la lógica de negocio respecto a las tecnologías externas.

El diseño incorpora patrones avanzados para maximizar la relevancia y el rendimiento:
*   **Pipeline de Ingesta Configurable:** Un proceso de ingesta modular que permite intercambiar estrategias de división de documentos (`chunking`) y encadenar "enriquecedores" de metadatos que operan en paralelo para etiquetar, clasificar y añadir contexto a los datos.
*   **Pipeline de Consulta Inteligente:** Un pipeline de consulta que implementa técnicas de vanguardia como la reescritura de consultas (`query rewriting`), la auto-consulta para filtrado de metadatos (`self-querying retrieval`), y el re-ranking de resultados para máxima precisión.
*   **Concurrencia sin Miedo:** Aprovecha al máximo el modelo de concurrencia de Rust para operaciones masivamente paralelas, tanto en tareas limitadas por CPU (con `rayon`) como por I/O (con `tokio`), garantizando un rendimiento excepcional.

El resultado es un framework RAG que no solo compite con las soluciones existentes, sino que las supera en rendimiento, fiabilidad, control y eficiencia de recursos, ideal para entornos de producción exigentes.

---

## 2. Principios Arquitectónicos
*   **Arquitectura Hexagonal (Puertos y Adaptadores):** El núcleo del sistema (dominio) es puro y no depende de ningún framework o servicio externo. Se comunica con el mundo exterior a través de "puertos" (traits de Rust), que son implementados por "adaptadores" (structs concretas) en la capa de infraestructura.
*   **Patrón de Pipeline (Cadena de Responsabilidad):** Tanto la ingesta como la consulta se modelan como pipelines secuenciales de etapas. Cada etapa es un componente aislado y componible, permitiendo que el flujo de trabajo sea fácilmente modificable y extensible vía configuración.
*   **Concurrencia Nativa:** El diseño está pensado desde la base para explotar la concurrencia. Las etapas del pipeline, aunque secuenciales en su orden, procesan los datos internamente de forma masivamente paralela.

---

## 3. Diagrama de Arquitectura
El sistema se divide en dos flujos principales: el Pipeline de Ingesta y el Pipeline de Consulta (RAG), donde la metadata enriquecida en la ingesta es utilizada activamente por el pipeline de consulta.

![Diagrama de Flujo de Metadatos Activos](https://i.imgur.com/83p1y3s.png)

---

## 4. Diseño Detallado del Dominio
El dominio contiene las reglas de negocio y las estructuras de datos puras, agnósticas a la implementación.

#### Entidades y Estructuras de Datos
```rust
// Representa un documento fuente.
pub struct Document { /* ... */ }

// Representa un chunk ANTES de ser vectorizado.
#[derive(Debug, Clone)]
pub struct ProtoChunk {
    pub document_id: DocumentId,
    pub content: String,
    pub metadata: HashMap<String, String>,
}

// Representa un chunk final, listo para ser almacenado.
#[derive(Debug, Clone)]
pub struct Chunk {
    pub id: ChunkId,
    pub document_id: DocumentId,
    pub content: String,
    pub embedding: Embedding,
    pub metadata: HashMap<String, String>,
}
```

#### Puertos (Traits)
```rust
// Puerto para dividir documentos
#[async_trait]
pub trait DocumentSplitter: Send + Sync {
    async fn split(&self, document: &Document) -> Result<Vec<ProtoChunk>, ChunkingError>;
}

// Puerto para enriquecer metadatos
#[async_trait]
pub trait MetadataEnricher: Send + Sync {
    async fn enrich(&self, proto_chunks: &mut Vec<ProtoChunk>) -> Result<(), MetadataError>;
}

// Puerto para modelos de embedding
#[async_trait]
pub trait EmbeddingModelRepository: Send + Sync { /* ... */ }

// Puerto para bases de datos vectoriales (con filtrado)
#[async_trait]
pub trait VectorStoreRepository: Send + Sync {
    async fn query(
        &self,
        collection_name: &str,
        query_embedding: Embedding,
        top_k: usize,
        filter: Option<MetadataFilter>, // Soporte para pre-filtrado
    ) -> Result<Vec<QueryResult>, VectorStoreError>;
    // ... otros métodos: upsert, delete, etc.
}
```

---

## 5. Diseño Detallado de la Aplicación
La capa de aplicación orquesta la lógica del dominio a través de dos pipelines principales.

#### Pipeline de Ingesta (`IngestionPipeline`)
Orquesta la división, enriquecimiento y almacenamiento de documentos de forma concurrente.

```rust
pub struct IngestionPipeline {
    splitter: Arc<dyn DocumentSplitter>,
    enrichers: Vec<Arc<dyn MetadataEnricher>>,
    embedding_repo: Arc<dyn EmbeddingModelRepository>,
    vector_store_repo: Arc<dyn VectorStoreRepository>,
}

impl IngestionPipeline {
    pub async fn execute(&self, document: Document) -> Result<(), anyhow::Error> {
        // 1. SPLIT: Divide el documento.
        let mut proto_chunks = self.splitter.split(&document).await?;
        
        // 2. ENRICH: Bucle secuencial, pero cada .enrich() es internamente paralelo.
        for enricher in &self.enrichers {
            enricher.enrich(&mut proto_chunks).await?;
        }
        
        // 3. EMBED & STORE: El resto del flujo, también con concurrencia interna.
        // ...
        Ok(())
    }
}
```

#### Pipeline de Consulta (`RagPipeline`)
Orquesta el flujo de consulta, aplicando técnicas avanzadas para mejorar la relevancia.

```rust
// Estado que fluye a través del pipeline
pub struct RagState { /* ... initial_query, processed_query, retrieved_chunks, ... */ }

// Trait para cada etapa del pipeline
#[async_trait]
pub trait RagStage: Send + Sync {
    async fn execute(&self, state: &mut RagState) -> Result<(), anyhow::Error>;
}

// El orquestador principal
pub struct RagPipeline {
    stages: Vec<Box<dyn RagStage>>,
}

impl RagPipeline {
    pub async fn execute(&self, query: String) -> Result<RagState, anyhow::Error> {
        let mut state = RagState::new(query);
        for stage in &self.stages {
            stage.execute(&mut state).await?;
        }
        Ok(state)
    }
}
```
**Etapas Configurables:** `QueryRewriteStage`, `RetrievalStage` (con self-querying), `RerankingStage`, `PromptFormatStage`, `GenerationStage`.

---

## 6. Diseño Detallado de la Infraestructura
La capa de infraestructura contiene las implementaciones concretas (adaptadores) de los puertos del dominio.

*   **Adaptadores `DocumentSplitter`:** `RecursiveCharacterSplitter`, `SemanticChunker`, etc.
*   **Adaptadores `MetadataEnricher`:** `TopicModelEnricher` (paralelo con `tokio` para llamadas a API), `SourceLinkageEnricher` (paralelo con `rayon` para operaciones de CPU).
*   **Adaptadores `VectorStoreRepository`:** `ChromaAdapter`, `PineconeAdapter`, etc., que traducen la estructura `MetadataFilter` a la sintaxis de consulta específica de cada proveedor.

---

## 7. Gestión de Configuración
Un archivo `config.toml` permite controlar cada aspecto del sistema sin recompilar.

```toml
[rag_system.ingestion_pipeline]
# Define el orden de los enriquecedores
enrichers = ["source_linkage", "topic_model"]

[rag_system.query_pipeline]
# Activa o desactiva etapas
enable_query_rewriting = true
enable_reranking = true

[rag_system.chunking]
strategy = "recursive_character"
# ... params para cada estrategia

[rag_system.enrichers.topic_model]
provider = "openai"
model = "gpt-4-turbo"

# ... configuración de embedding, vector_store, etc.
```

---

## 8. Estructura de Directorios
La estructura de carpetas refleja claramente la separación de responsabilidades impuesta por la arquitectura hexagonal.

```
src/
├── main.rs
├── config.rs
├── domain/
│   ├── mod.rs
│   ├── document.rs
│   ├── chunk.rs
│   ├── document_splitter.rs
│   ├── metadata_enricher.rs
│   └── (otros puertos y entidades)
├── application/
│   ├── mod.rs
│   ├── ingestion_pipeline.rs
│   └── query_pipeline/
│       ├── mod.rs
│       ├── state.rs
│       ├── stage.rs
│       └── (implementaciones de etapas)
└── infrastructure/
    ├── mod.rs
    ├── factories.rs
    ├── splitters/
    ├── enrichers/
    ├── vector_stores/
    └── embedding_models/
```
