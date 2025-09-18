# Guía Completa de Arquitectura Hexagonal

## Tabla de Contenido
1. [Introducción a la Arquitectura Hexagonal](#introducción-a-la-arquitectura-hexagonal)
2. [Estructura del Proyecto](#estructura-del-proyecto)
3. [Capas de la Arquitectura](#capas-de-la-arquitectura)
4. [Implementación Detallada](#implementación-detallada)
5. [Patrones y Principios](#patrones-y-principios)
6. [Guía para Implementar en Rust/Cargo](#guía-para-implementar-en-rustcargo)

## Introducción a la Arquitectura Hexagonal

La **Arquitectura Hexagonal** (también conocida como **Ports and Adapters**) es un patrón arquitectónico que busca aislar la lógica de negocio de las dependencias externas. Su objetivo principal es crear aplicaciones que sean:

- **Independientes de frameworks**
- **Testeable**
- **Independientes de la UI**
- **Independientes de la base de datos**
- **Independientes de servicios externos**

### Conceptos Clave

1. **Centro (Hexágono)**: Contiene la lógica de negocio pura
2. **Puertos (Ports)**: Interfaces que definen contratos
3. **Adaptadores (Adapters)**: Implementaciones concretas de los puertos

## Estructura del Proyecto

Basándome en el repositorio analizado, la estructura sigue este patrón:

```
src/
├── lib/
│   ├── Shared/
│   │   └── infrastructure/
│   │       └── ServiceContainer.ts
│   └── User/
│       ├── domain/           # CAPA DE DOMINIO (núcleo)
│       │   ├── User.ts
│       │   ├── UserId.ts
│       │   ├── UserName.ts
│       │   ├── UserEmail.ts
│       │   ├── UserCreatedAt.ts
│       │   ├── UserRepository.ts    # Puerto (interface)
│       │   └── UserNotFoundError.ts
│       ├── application/      # CAPA DE APLICACIÓN (casos de uso)
│       │   ├── UserCreate/
│       │   │   └── UserCreate.ts
│       │   ├── UserGetAll/
│       │   │   └── UserGetAll.ts
│       │   ├── UserGetOneById/
│       │   │   └── UserGetOneById.ts
│       │   ├── UserEdit/
│       │   │   └── UserEdit.ts
│       │   └── UserDelete/
│       │       └── UserDelete.ts
│       └── infrastructure/   # CAPA DE INFRAESTRUCTURA (adaptadores)
│           ├── InMemoryUserRepository.ts
│           ├── PostgresUserRepository.ts
│           └── ExpressUserController.ts
└── main.ts
```

## Capas de la Arquitectura

### 1. Capa de Dominio (Domain Layer)

**Ubicación**: `src/lib/User/domain/`
**Responsabilidad**: Contiene la lógica de negocio pura, entidades y reglas de dominio.

#### Características:
- **No depende de ninguna capa externa**
- Contiene las entidades del negocio
- Define los contratos (interfaces/puertos)
- Implementa las reglas de negocio

#### Implementación Detallada:

##### Value Objects (Objetos de Valor)

Los Value Objects encapsulan datos primitivos y sus validaciones:

```typescript
// UserId.ts
export class UserId {
  value: string;

  constructor(value: string) {
    this.value = value;
    this.ensureIsValid();
  }

  private ensureIsValid() {
    if (this.value.length < 5) {
      throw new Error("UserId must be at least 5 characters long");
    }
  }
}
```

**Principios de Value Objects**:
- **Inmutabilidad**: Una vez creados, no cambian
- **Validación en construcción**: Validan en el constructor
- **Igualdad por valor**: Se comparan por su valor, no por referencia
- **Sin identidad**: No tienen ID único

##### Entidades (Entities)

```typescript
// User.ts
export class User {
  id: UserId;
  name: UserName;
  email: UserEmail;
  createdAt: UserCreatedAt;

  constructor(
    id: UserId,
    name: UserName,
    email: UserEmail,
    createdAt: UserCreatedAt
  ) {
    this.id = id;
    this.name = name;
    this.email = email;
    this.createdAt = createdAt;
  }

  public nameAndEmail() {
    return `${this.name} - ${this.email}`;
  }
}
```

**Principios de Entidades**:
- **Identidad única**: Tienen un identificador único
- **Comportamiento rico**: Contienen lógica de negocio
- **Consistencia**: Mantienen su estado consistente

##### Puertos (Interfaces)

```typescript
// UserRepository.ts - PUERTO
export interface UserRepository {
  create(user: User): Promise<void>;
  getAll(): Promise<User[]>;
  getOneById(id: UserId): Promise<User | null>;
  edit(user: User): Promise<void>;
  delete(id: UserId): Promise<void>;
}
```

**Principios de Puertos**:
- Definen **QUÉ** se puede hacer, no **CÓMO**
- Son independientes de la implementación
- Permiten múltiples implementaciones (adaptadores)

##### Excepciones de Dominio

```typescript
// UserNotFoundError.ts
export class UserNotFoundError extends Error {}
```

### 2. Capa de Aplicación (Application Layer)

**Ubicación**: `src/lib/User/application/`
**Responsabilidad**: Orquesta las operaciones de dominio, implementa casos de uso.

#### Características:
- **Depende solo del dominio**
- Orquesta entidades y servicios de dominio
- Implementa casos de uso específicos
- No contiene lógica de negocio, solo coordinación

#### Implementación Detallada:

##### Casos de Uso (Use Cases)

```typescript
// UserCreate.ts
export class UserCreate {
  constructor(private repository: UserRepository) {}

  async run(
    id: string,
    name: string,
    email: string,
    createdAt: Date
  ): Promise<void> {
    // 1. Crear objetos de dominio con validaciones
    const user = new User(
      new UserId(id),
      new UserName(name),
      new UserEmail(email),
      new UserCreatedAt(createdAt)
    );

    // 2. Delegar al repositorio
    return this.repository.create(user);
  }
}
```

**Principios de Casos de Uso**:
- **Una responsabilidad por caso de uso**
- **Inyección de dependencias**: Reciben puertos por constructor
- **Orquestación**: Coordinan pero no contienen lógica de negocio
- **Independientes de infraestructura**

##### Patrón Command

Cada caso de uso sigue el patrón Command:

```typescript
// UserGetOneById.ts
export class UserGetOneById {
  constructor(private repository: UserRepository) {}

  async run(id: string): Promise<User> {
    const user = await this.repository.getOneById(new UserId(id));

    if (!user) throw new UserNotFoundError("User not found");

    return user;
  }
}
```

### 3. Capa de Infraestructura (Infrastructure Layer)

**Ubicación**: `src/lib/User/infrastructure/`
**Responsabilidad**: Implementa los puertos definidos en el dominio, contiene adaptadores.

#### Características:
- **Depende del dominio y aplicación**
- Implementa interfaces definidas en el dominio
- Contiene detalles técnicos específicos
- Es la capa más externa

#### Implementación Detallada:

##### Adaptadores de Persistencia

```typescript
// InMemoryUserRepository.ts - ADAPTADOR
export class InMemoryUserRepository implements UserRepository {
  private users: User[] = [];

  async create(user: User): Promise<void> {
    this.users.push(user);
  }

  async getOneById(id: UserId): Promise<User | null> {
    return this.users.find((user) => user.id.value === id.value) || null;
  }

  // ... otras implementaciones
}
```

##### Adaptadores de Base de Datos

```typescript
// PostgresUserRepository.ts
export class PostgresUserRepository implements UserRepository {
  client: Pool;

  constructor(databaseUrl: string) {
    this.client = new Pool({
      connectionString: databaseUrl,
    });
  }

  async create(user: User): Promise<void> {
    const query = {
      text: "INSERT INTO users (id, name, email) VALUES ($1, $2, $3)",
      values: [user.id.value, user.name.value, user.email.value],
    };

    await this.client.query(query);
  }

  // Mapeo de datos de base de datos a dominio
  private mapToDomain(user: PostgresUser): User {
    return new User(
      new UserId(user.id),
      new UserName(user.name),
      new UserEmail(user.email),
      new UserCreatedAt(user.created_at)
    );
  }
}
```

**Principios de Adaptadores de Persistencia**:
- **Implementan puertos del dominio**
- **Mapean entre representaciones**: Base de datos ↔ Dominio
- **Aíslan detalles técnicos**

##### Adaptadores de Interfaz (Controllers)

```typescript
// ExpressUserController.ts
export class ExpressUserController {
  async create(req: Request, res: Response) {
    const { createdAt, email, id, name } = req.body;

    await ServiceContainer.user.create.run(
      id,
      name,
      email,
      new Date(createdAt)
    );

    return res.status(201).send();
  }

  async getOneById(req: Request, res: Response) {
    try {
      const user = await ServiceContainer.user.getOneById.run(req.params.id);
      return res.json(user).status(200);
    } catch (error) {
      if (error instanceof UserNotFoundError) {
        return res.status(404).json({ message: error.message });
      }
      throw error;
    }
  }
}
```

**Principios de Controladores**:
- **Traducen entre HTTP y dominio**
- **Manejo de errores específicos**
- **No contienen lógica de negocio**
- **Delegan a casos de uso**

## Patrones y Principios

### 1. Dependency Injection (Inyección de Dependencias)

```typescript
// ServiceContainer.ts
const userRepository = new InMemoryUserRepository();

export const ServiceContainer = {
  user: {
    getAll: new UserGetAll(userRepository),
    getOneById: new UserGetOneById(userRepository),
    create: new UserCreate(userRepository),
    edit: new UserEdit(userRepository),
    delete: new UserDelete(userRepository),
  },
};
```

### 2. SOLID Principles

#### Single Responsibility Principle (SRP)
- Cada clase tiene una única responsabilidad
- `UserCreate` solo crea usuarios
- `UserId` solo valida IDs de usuario

#### Open/Closed Principle (OCP)
- Abierto para extensión, cerrado para modificación
- Nuevos adaptadores sin modificar dominio

#### Liskov Substitution Principle (LSP)
- `InMemoryUserRepository` y `PostgresUserRepository` son intercambiables

#### Interface Segregation Principle (ISP)
- Interfaces específicas y cohesivas
- `UserRepository` solo define operaciones de usuario

#### Dependency Inversion Principle (DIP)
- Dependencia de abstracciones, no concreciones
- Casos de uso dependen de `UserRepository` (interface), no de implementaciones específicas

### 3. Domain-Driven Design (DDD)

#### Ubiquitous Language
- `User`, `UserId`, `UserName` reflejan el lenguaje del negocio

#### Value Objects vs Entities
- **Value Objects**: `UserId`, `UserName`, `UserEmail` (inmutables, sin identidad)
- **Entities**: `User` (con identidad, mutable)

## Guía para Implementar en Rust/Cargo

### Estructura de Proyecto Rust

```
src/
├── lib.rs
├── shared/
│   └── infrastructure/
│       └── service_container.rs
└── user/
    ├── mod.rs
    ├── domain/
    │   ├── mod.rs
    │   ├── user.rs
    │   ├── user_id.rs
    │   ├── user_name.rs
    │   ├── user_email.rs
    │   ├── user_created_at.rs
    │   ├── user_repository.rs
    │   └── user_not_found_error.rs
    ├── application/
    │   ├── mod.rs
    │   ├── user_create.rs
    │   ├── user_get_all.rs
    │   ├── user_get_one_by_id.rs
    │   ├── user_edit.rs
    │   └── user_delete.rs
    └── infrastructure/
        ├── mod.rs
        ├── in_memory_user_repository.rs
        ├── postgres_user_repository.rs
        └── axum_user_controller.rs
```

### Implementación en Rust

#### 1. Cargo.toml

```toml
[package]
name = "hexagonal-architecture"
version = "0.1.0"
edition = "2021"

[dependencies]
tokio = { version = "1.0", features = ["full"] }
serde = { version = "1.0", features = ["derive"] }
uuid = { version = "1.0", features = ["v4"] }
thiserror = "1.0"
async-trait = "0.1"
axum = "0.7"
sqlx = { version = "0.7", features = ["postgres", "runtime-tokio-rustls", "chrono", "uuid"] }
chrono = { version = "0.4", features = ["serde"] }
```

#### 2. Value Objects en Rust

```rust
// user_id.rs
use serde::{Deserialize, Serialize};
use std::fmt::{Display, Formatter};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct UserId {
    value: String,
}

impl UserId {
    pub fn new(value: String) -> Result<Self, String> {
        if value.len() < 5 {
            return Err("UserId must be at least 5 characters long".to_string());
        }
        Ok(Self { value })
    }

    pub fn value(&self) -> &str {
        &self.value
    }
}

impl Display for UserId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.value)
    }
}
```

```rust
// user_email.rs
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserEmail {
    value: String,
}

impl UserEmail {
    pub fn new(value: String) -> Result<Self, String> {
        if !value.contains('@') || !value.contains('.') {
            return Err("UserEmail must be a valid email address".to_string());
        }
        Ok(Self { value })
    }

    pub fn value(&self) -> &str {
        &self.value
    }
}
```

#### 3. Entidad User en Rust

```rust
// user.rs
use crate::user::domain::{UserId, UserName, UserEmail, UserCreatedAt};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct User {
    pub id: UserId,
    pub name: UserName,
    pub email: UserEmail,
    pub created_at: UserCreatedAt,
}

impl User {
    pub fn new(
        id: UserId,
        name: UserName,
        email: UserEmail,
        created_at: UserCreatedAt,
    ) -> Self {
        Self {
            id,
            name,
            email,
            created_at,
        }
    }

    pub fn name_and_email(&self) -> String {
        format!("{} - {}", self.name.value(), self.email.value())
    }
}
```

#### 4. Puerto (Trait) en Rust

```rust
// user_repository.rs
use crate::user::domain::{User, UserId};
use async_trait::async_trait;
use std::error::Error;

#[async_trait]
pub trait UserRepository: Send + Sync {
    async fn create(&self, user: User) -> Result<(), Box<dyn Error>>;
    async fn get_all(&self) -> Result<Vec<User>, Box<dyn Error>>;
    async fn get_one_by_id(&self, id: &UserId) -> Result<Option<User>, Box<dyn Error>>;
    async fn edit(&self, user: User) -> Result<(), Box<dyn Error>>;
    async fn delete(&self, id: &UserId) -> Result<(), Box<dyn Error>>;
}
```

#### 5. Casos de Uso en Rust

```rust
// user_create.rs
use crate::user::domain::{User, UserId, UserName, UserEmail, UserCreatedAt, UserRepository};
use std::sync::Arc;
use chrono::{DateTime, Utc};

pub struct UserCreate {
    repository: Arc<dyn UserRepository>,
}

impl UserCreate {
    pub fn new(repository: Arc<dyn UserRepository>) -> Self {
        Self { repository }
    }

    pub async fn run(
        &self,
        id: String,
        name: String,
        email: String,
        created_at: DateTime<Utc>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let user = User::new(
            UserId::new(id)?,
            UserName::new(name)?,
            UserEmail::new(email)?,
            UserCreatedAt::new(created_at)?,
        );

        self.repository.create(user).await
    }
}
```

#### 6. Adaptador en Rust

```rust
// in_memory_user_repository.rs
use crate::user::domain::{User, UserId, UserRepository};
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::RwLock;
use std::error::Error;

pub struct InMemoryUserRepository {
    users: RwLock<HashMap<String, User>>,
}

impl InMemoryUserRepository {
    pub fn new() -> Self {
        Self {
            users: RwLock::new(HashMap::new()),
        }
    }
}

#[async_trait]
impl UserRepository for InMemoryUserRepository {
    async fn create(&self, user: User) -> Result<(), Box<dyn Error>> {
        let mut users = self.users.write().unwrap();
        users.insert(user.id.value().to_string(), user);
        Ok(())
    }

    async fn get_all(&self) -> Result<Vec<User>, Box<dyn Error>> {
        let users = self.users.read().unwrap();
        Ok(users.values().cloned().collect())
    }

    async fn get_one_by_id(&self, id: &UserId) -> Result<Option<User>, Box<dyn Error>> {
        let users = self.users.read().unwrap();
        Ok(users.get(id.value()).cloned())
    }

    async fn edit(&self, user: User) -> Result<(), Box<dyn Error>> {
        let mut users = self.users.write().unwrap();
        users.insert(user.id.value().to_string(), user);
        Ok(())
    }

    async fn delete(&self, id: &UserId) -> Result<(), Box<dyn Error>> {
        let mut users = self.users.write().unwrap();
        users.remove(id.value());
        Ok(())
    }
}
```

#### 7. Controlador con Axum

```rust
// axum_user_controller.rs
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::Json,
    routing::{get, post, put, delete},
    Router,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use crate::user::application::*;

#[derive(Clone)]
pub struct UserController {
    user_create: Arc<UserCreate>,
    user_get_all: Arc<UserGetAll>,
    user_get_one_by_id: Arc<UserGetOneById>,
    user_edit: Arc<UserEdit>,
    user_delete: Arc<UserDelete>,
}

#[derive(Deserialize)]
pub struct CreateUserRequest {
    id: String,
    name: String,
    email: String,
}

impl UserController {
    pub fn new(
        user_create: Arc<UserCreate>,
        user_get_all: Arc<UserGetAll>,
        user_get_one_by_id: Arc<UserGetOneById>,
        user_edit: Arc<UserEdit>,
        user_delete: Arc<UserDelete>,
    ) -> Self {
        Self {
            user_create,
            user_get_all,
            user_get_one_by_id,
            user_edit,
            user_delete,
        }
    }

    pub fn routes() -> Router<UserController> {
        Router::new()
            .route("/users", get(Self::get_all))
            .route("/users/:id", get(Self::get_one_by_id))
            .route("/users", post(Self::create))
            .route("/users", put(Self::edit))
            .route("/users/:id", delete(Self::delete))
    }

    async fn create(
        State(controller): State<UserController>,
        Json(payload): Json<CreateUserRequest>,
    ) -> Result<StatusCode, StatusCode> {
        controller
            .user_create
            .run(
                payload.id,
                payload.name,
                payload.email,
                chrono::Utc::now(),
            )
            .await
            .map_err(|_| StatusCode::BAD_REQUEST)?;

        Ok(StatusCode::CREATED)
    }

    // ... otros métodos
}
```

#### 8. Dependency Injection en Rust

```rust
// service_container.rs
use std::sync::Arc;
use crate::user::{
    domain::UserRepository,
    application::*,
    infrastructure::InMemoryUserRepository,
};

pub struct ServiceContainer {
    pub user_create: Arc<UserCreate>,
    pub user_get_all: Arc<UserGetAll>,
    pub user_get_one_by_id: Arc<UserGetOneById>,
    pub user_edit: Arc<UserEdit>,
    pub user_delete: Arc<UserDelete>,
}

impl ServiceContainer {
    pub fn new() -> Self {
        let user_repository: Arc<dyn UserRepository> =
            Arc::new(InMemoryUserRepository::new());

        Self {
            user_create: Arc::new(UserCreate::new(user_repository.clone())),
            user_get_all: Arc::new(UserGetAll::new(user_repository.clone())),
            user_get_one_by_id: Arc::new(UserGetOneById::new(user_repository.clone())),
            user_edit: Arc::new(UserEdit::new(user_repository.clone())),
            user_delete: Arc::new(UserDelete::new(user_repository.clone())),
        }
    }
}
```

### Diferencias Clave entre TypeScript y Rust

| Aspecto | TypeScript | Rust |
|---------|------------|------|
| **Error Handling** | Exceptions | Result/Option types |
| **Memory Management** | GC | Ownership/Borrowing |
| **Async** | Promises/async-await | Futures/async-await |
| **Dependency Injection** | Classes/interfaces | Traits/Arc |
| **Immutability** | Por convención | Por defecto |
| **Validation** | Runtime | Compile-time + Runtime |

### Ventajas de Rust para Arquitectura Hexagonal

1. **Type Safety**: El sistema de tipos de Rust ayuda a enforcar contratos
2. **Memory Safety**: Eliminación de errores de memoria
3. **Performance**: Cero costo de abstracción
4. **Concurrency**: Safe concurrency por defecto
5. **Error Handling**: Manejo explícito de errores con Result<T, E>

## Conclusiones

La Arquitectura Hexagonal proporciona:

1. **Separación clara de responsabilidades**
2. **Testabilidad**: Fácil testing con mocks
3. **Flexibilidad**: Intercambio de implementaciones
4. **Mantenibilidad**: Código más limpio y organizado
5. **Independencia**: Lógica de negocio aislada

Esta arquitectura es especialmente útil en aplicaciones empresariales donde la lógica de negocio es compleja y requiere ser independiente de frameworks y tecnologías específicas.