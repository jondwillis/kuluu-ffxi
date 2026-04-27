use thiserror::Error;

#[derive(Error, Debug, Clone, Copy)]
pub enum XError {
    #[error("Operation failed.")]
    Failed,
    #[error("Operation in progress.")]
    InProgress,
    #[error("Input data is not recognized.")]
    WrongMagic,
    #[error("Input data is in wrong version.")]
    WrongVersion,
    #[error("Operation ran out of memory.")]
    OutOfMemory,
    #[error("An input parameter was invalid.")]
    InvalidParam,
    #[error("Result buffer for the query was too small to store all results.")]
    BufferTooSmall,
    #[error("Query ran out of nodes during search.")]
    OutOfNodes,
    #[error("Query did not reach the end location, returning best guess.")]
    PartialResult,
    #[error("A tile has already been assigned to the given x,y coordinate")]
    AlreadyOccupied,
}
