// ASP.NET Core controller with raw SQL string interpolation.
// Vulnerable: CWE-89 (SQL Injection).

using Microsoft.AspNetCore.Mvc;
using System.Data.SqlClient;

namespace Tameeni.Web.Controllers;

[ApiController]
[Route("[controller]")]
public class UsersController : ControllerBase
{
    private readonly string _connStr;

    public UsersController(IConfiguration cfg)
    {
        _connStr = cfg.GetConnectionString("Default")!;
    }

    [HttpGet("{id}")]
    public async Task<IActionResult> GetUser(string id)
    {
        // `id` comes straight from the URL path; interpolated into a raw SQL
        // string and executed via SqlCommand. Classic CWE-89 — a payload like
        // `1' OR '1'='1` returns arbitrary rows.
        var sql = $"SELECT Id, Email, Role FROM Users WHERE Id = '{id}'";

        await using var conn = new SqlConnection(_connStr);
        await conn.OpenAsync();
        await using var cmd = new SqlCommand(sql, conn);
        await using var reader = await cmd.ExecuteReaderAsync();

        if (!await reader.ReadAsync()) return NotFound();
        return Ok(new
        {
            Id = reader.GetInt64(0),
            Email = reader.GetString(1),
            Role = reader.GetString(2),
        });
    }
}
