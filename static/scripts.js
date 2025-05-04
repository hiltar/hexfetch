/* Light theme variables (default) */
:root {
    --bg-color: #ffffff;
    --text-color: #000000;
    --card-bg: #f8f9fa;
    --primary-bg: #007bff;
    --primary-text: #ffffff;
    --input-bg: #ffffff;
    --input-text: #000000;
    --nav-bg: #ffffff;
    --nav-text: #000000;
    --nav-active-bg: #e9ecef;
    --border-color: #dee2e6;
}

/* Dark theme variables */
[data-theme="dark"] {
    --bg-color: #212529;
    --text-color: #ffffff;
    --card-bg: #343a40;
    --primary-bg: #0056b3;
    --primary-text: #ffffff;
    --input-bg: #495057;
    --input-text: #ffffff;
    --nav-bg: #343a40;
    --nav-text: #ffffff;
    --nav-active-bg: #495057;
    --border-color: #6c757d;
}

/* General styles */
body {
    background-color: var(--bg-color);
    color: var(--text-color);
}

.container {
    background-color: var(--card-bg);
    padding: 20px;
    border-radius: 8px;
    border: 1px solid var(--border-color);
}

.nav-tabs {
    background-color: var(--nav-bg);
    border-bottom: 1px solid var(--border-color);
}

.nav-link {
    color: var(--nav-text);
}

.nav-link.active {
    background-color: var(--nav-active-bg);
    color: var(--text-color);
    border-color: var(--border-color);
}

.btn-primary {
    background-color: var(--primary-bg);
    border-color: var(--primary-bg);
    color: var(--primary-text);
}

.btn-primary:hover {
    background-color: #0056b3; /* Slightly darker for hover */
    border-color: #0056b3;
}

.btn-danger {
    background-color: #dc3545;
    border-color: #dc3545;
}

.btn-danger:hover {
    background-color: #c82333;
    border-color: #c82333;
}

.form-control {
    background-color: var(--input-bg);
    color: var(--input-text);
    border-color: var(--border-color);
}

.form-control:focus {
    background-color: var(--input-bg);
    color: var(--input-text);
    border-color: var(--primary-bg);
    box-shadow: 0 0 0 0.2rem rgba(0, 123, 255, 0.25);
}

.live-data p {
    text-align: center;
    font-weight: bold;
    font-size: 24px; /* Preserved from previous requirement */
    margin: 10px 0;
    color: var(--text-color);
}

#hexChart {
    max-width: 600px;
    margin: 0 auto;
    background-color: var(--card-bg);
    padding: 10px;
    border-radius: 8px;
    border: 1px solid var(--border-color);
}

.miner-item {
    display: flex;
    justify-content: space-between;
    align-items: center;
    margin-bottom: 10px;
    color: var(--text-color);
}

/* Ensure icons align with text */
.nav-link i {
    vertical-align: middle;
    margin-right: 5px;
}

.btn i {
    vertical-align: middle;
}

/* Ensure datepicker z-index and theme compatibility */
.datepicker {
    z-index: 1050;
    background-color: var(--card-bg);
    color: var(--text-color);
}

.datepicker table tr td,
.datepicker table tr th {
    color: var(--text-color);
}

.datepicker .datepicker-days .table-condensed {
    background-color: var(--card-bg);
}

/* Theme toggle switch */
.form-check-input:checked {
    background-color: var(--primary-bg);
    border-color: var(--primary-bg);
}

.form-check-label i {
    vertical-align: middle;
}

/* Modal styling */
.modal-content {
    background-color: var(--card-bg);
    color: var(--text-color);
    border-color: var(--border-color);
}

.modal-header,
.modal-footer {
    border-color: var(--border-color);
}
