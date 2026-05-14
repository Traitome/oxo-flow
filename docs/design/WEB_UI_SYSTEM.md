# oxo-flow Web UI System Design Specification

## Overview
The oxo-flow Web UI System is an industrial-grade bioinformatics workflow management portal. It extends the core `oxo-flow` engine into a multi-tenant, resource-aware environment designed for managing the full lifecycle of genomic pipelines.

## Core Architecture

### 1. Persistence & State Management
- **Database**: SQLite (via `sqlx`) for zero-config, single-binary deployment.
- **Scope**: Stores users, job metadata, audit trails, and OS account mappings.
- **Task Queue**: In-memory async queue backed by the SQLite state for crash recovery.

### 2. Multi-Tenancy & Security
- **Isolation Model**: Physical directory isolation for each user and run.
- **OS Account Binding**:
    - **Sudo Mode (Local)**: Web service executes tasks via `sudo -u <user>` with restricted command white-listing.
    - **SSH Mode (Remote/Secure)**: Execution via SSH/SFTP using user-provided private keys.
- **Permissions**: Leverages host POSIX permissions to enforce data access control.

### 3. Execution Sandbox
- **Base Directory**: `workspace/`
- **User Root**: `workspace/users/<username>/`
- **Run Directory**: `workspace/users/<username>/runs/<run_id>/`
- **Mechanism**: Every run copies the workflow template to a dedicated sandbox and executes with a unique checkpoint state.

### 4. Resource Sensing & Scheduling
- **Host Sensing**: Real-time monitoring of local CPU, Memory, and Disk usage via `sysinfo`.
- **HPC Sensing**: Periodic polling of cluster schedulers (SLURM `sinfo`, PBS `qstat`, etc.) to track queue availability.
- **Smart Scheduling**: Blocking or queuing tasks based on real-time resource constraints.

### 5. Lifecycle Management UI
- **Import/Generation**:
    - Interactive `.oxoflow` editor with linting.
    - Automatic sample discovery: Scan directories and generate sample sheets via regex.
- **Monitoring**:
    - **Dynamic DAG Visualization**: Real-time node status coloring (Pending, Running, Success, Failed).
    - **Streaming Logs**: Live standard output/error redirection to the browser.
- **Intervention**: UI controls for pausing, terminating, and resuming tasks using Checkpoints.

### 6. User Interface Details
- **File Browser**: Integrated, permission-aware file selector for choosing input paths.
- **Smart Hints**: Suggesting common data paths and auto-completing wildcards.
- **Audit Log**: Every host-level action (run, delete, export) is recorded with a timestamp and user ID.
- **Notifications**: Async alerts via Email/Webhook upon task completion or failure.

## User Roles
- **Viewer**: Read-only access to dashboards and public reports.
- **User**: Manage private workflows, runs, and datasets.
- **Admin**: System configuration, cluster profile management, resource monitoring, and audit log access.
